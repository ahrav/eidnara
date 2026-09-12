//! Durable dispatch state for embedding jobs: the lane the projection admits
//! work to, the pending rows eligible for admission, and the attempt ledger
//! every disposition is charged against.
//!
//! An episode is one finite grant of attempts under one deadline. Admission
//! consumes an attempt; a retryable failure returns the row to pending under
//! the same episode; exhaustion, an expired deadline, or a terminal failure
//! records a stop reason that holds the row until an explicit authorization
//! reference opens a new episode. Reopening the projection never replenishes
//! attempts, renews an episode, or grants authorization. Every function runs
//! inside the caller's transaction and names identities, never payload text.

use std::num::{NonZeroU32, NonZeroUsize};

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::ProjectionError;
use crate::batch::VectorGeneration;
use crate::vectors::Obsoletion;

/// The verified lane identity and the host incarnation that serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneBinding {
    pub embedding_model: String,
    pub bundle_fingerprint: String,
    pub vector_dimension: u32,
    pub table_epoch: u64,
    pub host_incarnation: String,
}

impl LaneBinding {
    /// Whether a registered generation was produced under this lane's model identity.
    pub fn serves(&self, generation: &VectorGeneration) -> bool {
        self.embedding_model == generation.embedding_model
            && self.bundle_fingerprint == generation.tokenizer_fingerprint
            && self.vector_dimension == generation.vector_dimension
            && self.table_epoch == generation.generation_epoch
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingOutcome {
    /// The lane is the product the projection was built for, and every admitted row is held by this host.
    Bound,
    /// The lane matches but `released` admitted rows were held by another host incarnation; they are pending again under their episodes.
    Rebound { released: usize },
    /// The projection was built for a different product; nothing is admitted until an operator rebuilds it.
    Mismatch,
    /// No projection identity is installed, so there is no product to bind to.
    Unbuilt,
}

/// Binds dispatch to `lane`: the lane must be the product the projection
/// identity was built for, and admitted rows held by a host incarnation other
/// than the lane's are unreachable, so they return to pending with their
/// attempts kept.
pub fn bind_lane(
    conn: &GuardedConn<'_>,
    lane: &LaneBinding,
    now: i64,
) -> Result<BindingOutcome, ProjectionError> {
    let Some(identity) = crate::read_identity(conn)? else {
        return Ok(BindingOutcome::Unbuilt);
    };
    let built_for = (
        identity.embedding_model.as_str(),
        identity.tokenizer_fingerprint.as_str(),
        identity.vector_dimension,
        identity.generation_epoch,
    );
    let served = (
        lane.embedding_model.as_str(),
        lane.bundle_fingerprint.as_str(),
        lane.vector_dimension,
        lane.table_epoch,
    );
    if built_for != served {
        return Ok(BindingOutcome::Mismatch);
    }
    let released = conn.execute(
        "UPDATE embedding_jobs SET state='pending',host_job_id=NULL,host_incarnation=NULL,last_failure_kind='host_restarted',updated_at=?2
         WHERE state='admitted' AND host_incarnation IS NOT ?1",
        params![lane.host_incarnation, now],
    )?;
    Ok(if released == 0 {
        BindingOutcome::Bound
    } else {
        BindingOutcome::Rebound { released }
    })
}

/// One finite grant of attempts under one deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeGrant {
    pub allowance: NonZeroU32,
    pub deadline: i64,
}

/// The episode a row runs under. An episode is open only when its id and deadline are both stored; a row without one stores allowance 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    /// The host item identity the row is admitted and polled under.
    pub id: String,
    pub allowance: u32,
    pub deadline: i64,
}

impl Episode {
    fn from_columns(
        id: Option<String>,
        allowance: u32,
        deadline: Option<i64>,
    ) -> Result<Option<Self>, ProjectionError> {
        match (id, deadline) {
            (Some(id), Some(deadline)) => Ok(Some(Self {
                id,
                allowance,
                deadline,
            })),
            (None, None) if allowance == 0 => Ok(None),
            _ => Err(ProjectionError::CorruptRow),
        }
    }
}

/// The episode a row's first admission opens.
pub fn first_episode_id(job_id: &str) -> String {
    format!("{job_id}/1")
}

/// Authorized episodes have their own namespace, so no reference can name the first episode.
fn authorized_episode_id(job_id: &str, authorization_ref: &str) -> String {
    format!("{job_id}/auth/{authorization_ref}")
}

/// A job row the dispatcher may act on, with the input it embeds and the
/// current-input identity the completion is guarded by.
#[derive(Clone, PartialEq, Eq)]
pub struct DispatchJob {
    pub job_id: String,
    pub occurrence_id: String,
    pub generation: VectorGeneration,
    pub payload_id: String,
    pub source_object_id: String,
    pub revision: i64,
    pub source_artifact_digest: String,
    pub text: String,
    pub state: String,
    pub attempts: u32,
    /// A row has no open episode until its first admission.
    pub episode: Option<Episode>,
    pub host_job_id: Option<String>,
}

impl std::fmt::Debug for DispatchJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchJob")
            .field("job_id", &self.job_id)
            .field("occurrence_id", &self.occurrence_id)
            .field("generation", &self.generation)
            .field("payload_id", &self.payload_id)
            .field("source_object_id", &self.source_object_id)
            .field("revision", &self.revision)
            .field("source_artifact_digest", &self.source_artifact_digest)
            .field("text_bytes", &self.text.len())
            .field("state", &self.state)
            .field("attempts", &self.attempts)
            .field("episode", &self.episode)
            .field("host_job_id", &self.host_job_id)
            .finish()
    }
}

impl DispatchJob {
    /// The host item identity is the open episode, or the first episode the row's admission opens.
    pub fn item_id(&self) -> String {
        match &self.episode {
            Some(episode) => episode.id.clone(),
            None => first_episode_id(&self.job_id),
        }
    }

    /// Returns the reason no further attempt may be admitted at `now`; a row without an episode uses the grant that admission would open.
    pub fn episode_refusal(&self, grant: EpisodeGrant, now: i64) -> Option<&'static str> {
        refusal(self.attempts, self.episode.as_ref(), grant, now)
    }

    /// An admitted row's charged attempt expires only at its deadline; exhaustion refuses new attempts, not this one.
    pub fn completion_refusal(&self, grant: EpisodeGrant, now: i64) -> Option<&'static str> {
        let (_, deadline) = bounds(self.episode.as_ref(), grant);
        (now > deadline).then_some(DEADLINE_EXPIRED)
    }
}

fn bounds(episode: Option<&Episode>, grant: EpisodeGrant) -> (u32, i64) {
    match episode {
        Some(episode) => (episode.allowance, episode.deadline),
        None => (grant.allowance.get(), grant.deadline),
    }
}

fn refusal(
    attempts: u32,
    episode: Option<&Episode>,
    grant: EpisodeGrant,
    now: i64,
) -> Option<&'static str> {
    let (allowance, deadline) = bounds(episode, grant);
    if attempts >= allowance {
        Some(EXHAUSTED)
    } else if now > deadline {
        Some(DEADLINE_EXPIRED)
    } else {
        None
    }
}

/// Stores the exclusive `(created_at, job_id)` position for keyset pagination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchCursor {
    pub created_at: i64,
    pub job_id: String,
}

/// Metadata for judging a job without loading its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchCandidate {
    pub job_id: String,
    pub occurrence_id: String,
    pub generation_id: String,
    pub source_object_id: String,
    pub source_revision: i64,
    pub source_artifact_digest: String,
    pub created_at: i64,
    pub readiness: CandidateReadiness,
}

impl DispatchCandidate {
    pub fn cursor(&self) -> DispatchCursor {
        DispatchCursor {
            created_at: self.created_at,
            job_id: self.job_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateReadiness {
    Ready,
    Deferred { until: i64 },
}

const OPEN_JOB_CANDIDATES_SQL: &str =
    "SELECT j.job_id,j.occurrence_id,j.generation_id,o.source_object_id,o.revision,
            o.source_artifact_digest,j.created_at,j.state,j.next_attempt_at,j.host_job_id
       FROM embedding_jobs AS j INDEXED BY idx_embedding_jobs_open_order
        LEFT JOIN occurrences AS o ON o.occurrence_id=j.occurrence_id
       WHERE j.state IN ('pending','admitted') AND j.stop_reason IS NULL
       ORDER BY j.created_at,j.job_id LIMIT ?1";

const OPEN_JOB_CANDIDATES_AFTER_SQL: &str =
    "SELECT j.job_id,j.occurrence_id,j.generation_id,o.source_object_id,o.revision,
            o.source_artifact_digest,j.created_at,j.state,j.next_attempt_at,j.host_job_id
       FROM embedding_jobs AS j INDEXED BY idx_embedding_jobs_open_order
        LEFT JOIN occurrences AS o ON o.occurrence_id=j.occurrence_id
       WHERE j.state IN ('pending','admitted') AND j.stop_reason IS NULL
        AND (j.created_at,j.job_id)>(?1,?2)
       ORDER BY j.created_at,j.job_id LIMIT ?3";

fn candidate(row: &rusqlite::Row<'_>, now: i64) -> Result<DispatchCandidate, ProjectionError> {
    let state: String = row.get(7)?;
    let next_attempt_at: Option<i64> = row.get(8)?;
    let host_job_id: Option<String> = row.get(9)?;
    let readiness = match state.as_str() {
        "pending" => match next_attempt_at {
            Some(until) if until > now => CandidateReadiness::Deferred { until },
            _ => CandidateReadiness::Ready,
        },
        "admitted" if host_job_id.is_some() => CandidateReadiness::Ready,
        _ => return Err(ProjectionError::CorruptRow),
    };
    Ok(DispatchCandidate {
        job_id: row.get(0)?,
        occurrence_id: row.get(1)?,
        generation_id: row.get(2)?,
        source_object_id: row
            .get::<_, Option<String>>(3)?
            .ok_or(ProjectionError::CorruptRow)?,
        source_revision: row
            .get::<_, Option<i64>>(4)?
            .ok_or(ProjectionError::CorruptRow)?,
        source_artifact_digest: row
            .get::<_, Option<String>>(5)?
            .ok_or(ProjectionError::CorruptRow)?,
        created_at: row.get(6)?,
        readiness,
    })
}

pub fn open_job_candidates(
    conn: &GuardedConn<'_>,
    cursor: Option<&DispatchCursor>,
    limit: NonZeroUsize,
    now: i64,
) -> Result<Vec<DispatchCandidate>, ProjectionError> {
    let limit = i64::try_from(limit.get()).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(if cursor.is_some() {
        OPEN_JOB_CANDIDATES_AFTER_SQL
    } else {
        OPEN_JOB_CANDIDATES_SQL
    })?;
    let mut rows = match cursor {
        Some(cursor) => statement.query(params![cursor.created_at, cursor.job_id, limit])?,
        None => statement.query([limit])?,
    };
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        candidates.push(candidate(row, now)?);
    }
    Ok(candidates)
}

pub fn dispatch_job(
    conn: &GuardedConn<'_>,
    job_id: &str,
    now: i64,
) -> Result<Option<DispatchJob>, ProjectionError> {
    let row = conn
        .query_row(
            "SELECT occurrence_id,generation_id,state,attempts,episode_allowance,episode_deadline,
                host_job_id,episode_id,next_attempt_at,stop_reason
             FROM embedding_jobs WHERE job_id=?1",
            [job_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?;
    let Some((
        occurrence_id,
        generation_id,
        state,
        attempts,
        allowance,
        deadline,
        host_job_id,
        episode_id,
        next_attempt_at,
        stop_reason,
    )) = row
    else {
        return Ok(None);
    };
    let generation = conn
        .query_row(
            "SELECT embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch
             FROM vector_generations WHERE generation_id=?1",
            [&generation_id],
            |row| {
                let epoch: i64 = row.get(3)?;
                Ok(VectorGeneration {
                    generation_id: generation_id.clone(),
                    embedding_model: row.get(0)?,
                    tokenizer_fingerprint: row.get(1)?,
                    vector_dimension: row.get(2)?,
                    generation_epoch: epoch.unsigned_abs(),
                })
            },
        )
        .optional()?
        .ok_or(ProjectionError::CorruptRow)?;
    let eligible = stop_reason.is_none()
        && ((state == "pending" && next_attempt_at.is_none_or(|retry| retry <= now))
            || (state == "admitted" && host_job_id.is_some()));
    if !eligible {
        return Ok(None);
    }
    let occurrence =
        crate::read_occurrence(conn, &occurrence_id)?.ok_or(ProjectionError::CorruptRow)?;
    Ok(Some(DispatchJob {
        job_id: job_id.to_owned(),
        occurrence_id: occurrence.occurrence_id,
        generation,
        payload_id: occurrence.payload_id,
        source_object_id: occurrence.source_object_id,
        revision: occurrence.revision,
        source_artifact_digest: occurrence.source_artifact_digest,
        text: String::from_utf8(occurrence.bytes).map_err(|_| ProjectionError::CorruptRow)?,
        state,
        attempts,
        episode: Episode::from_columns(episode_id, allowance, deadline)?,
        host_job_id,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// One attempt was charged and the row now names the host job.
    Charged { attempts: u32 },
    /// The same host job is already recorded, so a lost reply was not a lost charge.
    AlreadyCharged,
    /// The current row names a different episode than the submitted item, so the row is unchanged.
    EpisodeChanged,
    /// The episode has no attempt left or its deadline passed; the row is stopped with that reason.
    Stopped(&'static str),
    /// The row is not pending.
    NotPending,
}

/// Charges one attempt for admitting `job_id` to `host` as `host_job_id`.
/// `expected_episode_id` is the item identity submitted to the host.
/// A row without an episode opens one from `grant`; a row with an episode keeps
/// its allowance and deadline. The charge is refused, and the row stopped,
/// when the episode is exhausted or expired.
pub fn charge_admission(
    conn: &GuardedConn<'_>,
    job_id: &str,
    expected_episode_id: &str,
    host: &LaneBinding,
    host_job_id: &str,
    grant: EpisodeGrant,
    now: i64,
) -> Result<Admission, ProjectionError> {
    let Some(ledger) = job_ledger(conn, job_id)? else {
        return Ok(Admission::NotPending);
    };
    let episode = ledger.episode()?;
    let episode_matches = match &episode {
        Some(episode) => episode.id == expected_episode_id,
        None => first_episode_id(job_id) == expected_episode_id,
    };
    if !episode_matches {
        return Ok(Admission::EpisodeChanged);
    }
    if ledger.state == "admitted" && ledger.host_job_id.as_deref() == Some(host_job_id) {
        return Ok(Admission::AlreadyCharged);
    }
    if ledger.state != "pending" {
        return Ok(Admission::NotPending);
    }
    let attempts = ledger.attempts;
    if let Some(reason) = refusal(attempts, episode.as_ref(), grant, now) {
        stop_job(conn, job_id, reason, now)?;
        return Ok(Admission::Stopped(reason));
    }
    let episode = episode.unwrap_or_else(|| Episode {
        id: first_episode_id(job_id),
        allowance: grant.allowance.get(),
        deadline: grant.deadline,
    });
    let epoch = i64::try_from(host.table_epoch).map_err(|_| ProjectionError::CorruptRow)?;
    conn.execute(
        "UPDATE embedding_jobs SET state='admitted',attempts=attempts+1,host_job_id=?2,host_incarnation=?3,admitted_epoch=?4,
                episode_id=?5,episode_allowance=?6,episode_deadline=?7,next_attempt_at=NULL,updated_at=?8
         WHERE job_id=?1 AND state='pending'",
        params![
            job_id,
            host_job_id,
            host.host_incarnation,
            epoch,
            episode.id,
            episode.allowance,
            episode.deadline,
            now
        ],
    )?;
    Ok(Admission::Charged {
        attempts: attempts + 1,
    })
}

pub fn rebind_host_job(
    conn: &GuardedConn<'_>,
    job_id: &str,
    evicted_host_job_id: &str,
    host_job_id: &str,
    now: i64,
) -> Result<bool, ProjectionError> {
    let changed = conn.execute(
        "UPDATE embedding_jobs SET host_job_id=?3,updated_at=?4
         WHERE job_id=?1 AND state='admitted' AND host_job_id=?2",
        params![job_id, evicted_host_job_id, host_job_id, now],
    )?;
    Ok(changed == 1)
}

/// Stop reason recorded when an episode's attempts are all consumed.
pub const EXHAUSTED: &str = "exhausted";
/// Stop reason recorded when an episode's deadline passed before success.
pub const DEADLINE_EXPIRED: &str = "deadline_expired";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// The row is pending again under its episode, eligible at `retry_at`.
    Retry,
    /// The episode has no attempt left; the row is stopped as exhausted.
    Exhausted,
    /// The row was not open, so nothing was recorded.
    NotOpen,
}

/// Records a retryable failure: the row returns to pending under the same
/// episode, or stops as exhausted when the charged attempts already equal the
/// allowance. `kind` is a closed failure vocabulary, never payload text.
pub fn record_retry(
    conn: &GuardedConn<'_>,
    job_id: &str,
    kind: &str,
    retry_at: i64,
    now: i64,
) -> Result<Disposition, ProjectionError> {
    let Some(ledger) = job_ledger(conn, job_id)? else {
        return Ok(Disposition::NotOpen);
    };
    if ledger.state != "pending" && ledger.state != "admitted" {
        return Ok(Disposition::NotOpen);
    }
    // A row that never opened an episode has no allowance to exhaust yet.
    if let Some(episode) = ledger.episode()?
        && ledger.attempts >= episode.allowance
    {
        stop_job(conn, job_id, EXHAUSTED, now)?;
        return Ok(Disposition::Exhausted);
    }
    conn.execute(
        "UPDATE embedding_jobs SET state='pending',host_job_id=NULL,host_incarnation=NULL,last_failure_kind=?2,next_attempt_at=?3,updated_at=?4
         WHERE job_id=?1",
        params![job_id, kind, retry_at, now],
    )?;
    Ok(Disposition::Retry)
}

/// Stops automatic dispatch of an open row with `reason`. The row keeps its
/// episode and attempts; only [`authorize_recovery`] reopens it.
pub fn stop_job(
    conn: &GuardedConn<'_>,
    job_id: &str,
    reason: &str,
    now: i64,
) -> Result<bool, ProjectionError> {
    let changed = conn.execute(
        "UPDATE embedding_jobs SET state='failed',host_job_id=NULL,host_incarnation=NULL,last_failure_kind=?2,stop_reason=?2,updated_at=?3
         WHERE job_id=?1 AND state IN ('pending','admitted')",
        params![job_id, reason, now],
    )?;
    Ok(changed == 1)
}

pub fn obsolete_judged_job(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    generation_id: &str,
    source_object_id: &str,
    source_revision: i64,
    source_artifact_digest: &str,
    now: i64,
) -> Result<Obsoletion, ProjectionError> {
    let changed = conn.execute(
        "UPDATE embedding_jobs SET state='obsolete',updated_at=?6
         WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')
           AND EXISTS(SELECT 1 FROM occurrences o
                      WHERE o.occurrence_id=?1 AND o.source_object_id=?3
                        AND o.revision=?4 AND o.source_artifact_digest=?5)",
        params![
            occurrence_id,
            generation_id,
            source_object_id,
            source_revision,
            source_artifact_digest,
            now
        ],
    )?;
    if changed == 1 {
        return Ok(Obsoletion::Marked);
    }
    let state: Option<String> = conn
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2",
            params![occurrence_id, generation_id],
            |row| row.get(0),
        )
        .optional()?;
    match state.as_deref() {
        None => Ok(Obsoletion::NoJob),
        Some("pending" | "admitted") => Err(ProjectionError::IdentityMismatch),
        Some(_) => Ok(Obsoletion::AlreadyTerminal),
    }
}

/// The episode identity is the host's item identity, which the host bounds at 256 bytes: a 64-byte job identity, `/auth/`, and a 128-byte reference total 198.
pub const MAX_AUTHORIZATION_REF_BYTES: usize = 128;

/// One token of `[A-Za-z0-9._:-]`, so a reference carries no separator, whitespace, or control byte into the episode identity.
fn valid_authorization_ref(authorization_ref: &str) -> bool {
    !authorization_ref.is_empty()
        && authorization_ref.len() <= MAX_AUTHORIZATION_REF_BYTES
        && authorization_ref
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// A new episode is open under `authorization_ref`.
    Granted { episode_id: String },
    /// This reference is already consumed for this job; it grants nothing regardless of the current episode or state.
    Replayed,
    /// The row is not stopped.
    NotStopped,
    /// The reference is empty, over [`MAX_AUTHORIZATION_REF_BYTES`], or not one token of `[A-Za-z0-9._:-]`; nothing changed.
    InvalidReference,
}

/// Opens a new episode for a stopped row under an external authorization
/// reference. The episode identity is derived from the reference, so a
/// replayed authorization grants nothing twice.
pub fn authorize_recovery(
    conn: &GuardedConn<'_>,
    job_id: &str,
    authorization_ref: &str,
    grant: EpisodeGrant,
    now: i64,
) -> Result<Recovery, ProjectionError> {
    if !valid_authorization_ref(authorization_ref) {
        return Ok(Recovery::InvalidReference);
    }
    let consumed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM embedding_recovery_authorizations WHERE job_id=?1 AND authorization_ref=?2)",
        params![job_id, authorization_ref],
        |row| row.get(0),
    )?;
    if consumed {
        return Ok(Recovery::Replayed);
    }
    let Some(ledger) = job_ledger(conn, job_id)? else {
        return Ok(Recovery::NotStopped);
    };
    if ledger.state != "failed" {
        return Ok(Recovery::NotStopped);
    }
    let episode_id = authorized_episode_id(job_id, authorization_ref);
    conn.execute(
        "INSERT INTO embedding_recovery_authorizations(job_id,authorization_ref) VALUES(?1,?2)",
        params![job_id, authorization_ref],
    )?;
    conn.execute(
        "UPDATE embedding_jobs SET state='pending',attempts=0,episode_id=?2,episode_allowance=?3,episode_deadline=?4,
                authorization_ref=?5,stop_reason=NULL,last_failure_kind=NULL,next_attempt_at=NULL,host_job_id=NULL,host_incarnation=NULL,updated_at=?6
         WHERE job_id=?1",
        params![job_id, episode_id, grant.allowance.get(), grant.deadline, authorization_ref, now],
    )?;
    Ok(Recovery::Granted { episode_id })
}

/// The durable accounting of one job, for ledgers and operator inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLedger {
    pub state: String,
    pub attempts: u32,
    pub episode_id: Option<String>,
    pub episode_allowance: u32,
    pub episode_deadline: Option<i64>,
    pub host_job_id: Option<String>,
    pub host_incarnation: Option<String>,
    pub last_failure_kind: Option<String>,
    pub stop_reason: Option<String>,
    pub authorization_ref: Option<String>,
}

impl JobLedger {
    /// # Errors
    ///
    /// [`ProjectionError::CorruptRow`] when the episode columns disagree about whether an episode is open.
    pub fn episode(&self) -> Result<Option<Episode>, ProjectionError> {
        Episode::from_columns(
            self.episode_id.clone(),
            self.episode_allowance,
            self.episode_deadline,
        )
    }
}

pub fn job_ledger(
    conn: &GuardedConn<'_>,
    job_id: &str,
) -> Result<Option<JobLedger>, ProjectionError> {
    Ok(conn
        .query_row(
            "SELECT state,attempts,episode_id,episode_allowance,episode_deadline,host_job_id,host_incarnation,last_failure_kind,stop_reason,authorization_ref
             FROM embedding_jobs WHERE job_id=?1",
            [job_id],
            |row| {
                Ok(JobLedger {
                    state: row.get(0)?,
                    attempts: row.get(1)?,
                    episode_id: row.get(2)?,
                    episode_allowance: row.get(3)?,
                    episode_deadline: row.get(4)?,
                    host_job_id: row.get(5)?,
                    host_incarnation: row.get(6)?,
                    last_failure_kind: row.get(7)?,
                    stop_reason: row.get(8)?,
                    authorization_ref: row.get(9)?,
                })
            },
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::{OPEN_JOB_CANDIDATES_AFTER_SQL, OPEN_JOB_CANDIDATES_SQL};
    use rusqlite::{Connection, StatementStatus, params};

    #[test]
    fn open_identity_query_bounds_due_and_deferred_backlogs() {
        let mut measured = Vec::new();
        let mut keyset_measured = Vec::new();
        let mut deferred_measured = Vec::new();
        for count in [1_024, 16_384] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(crate::BASELINE).unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            conn.execute_batch(
                "INSERT INTO payloads VALUES ('p',x'61',1,0);
                 INSERT INTO vector_generations VALUES ('g','model','fp',8,1,'building',0,0);",
            )
            .unwrap();
            conn.execute(
                "WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<?1)
                 INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                     payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                     source_artifact_digest,created_commit_seq,persisted_at)
                 SELECT printf('%05d',n),x'00','lineage','messages',1,'text','p','domain','normal',
                     'source','evidence','digest',1,0 FROM ids",
                [count],
            ).unwrap();
            conn.execute(
                "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,created_at,updated_at)
                 SELECT occurrence_id,occurrence_id,'g','pending',(?1-CAST(occurrence_id AS INTEGER))/2,0
                 FROM occurrences",
                [count],
            ).unwrap();
            let mut statement = conn.prepare(OPEN_JOB_CANDIDATES_SQL).unwrap();
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {OPEN_JOB_CANDIDATES_SQL}"))
                .unwrap()
                .query_map([1], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                plan.iter()
                    .any(|detail| detail.contains("idx_embedding_jobs_open_order")),
                "ordered index missing from plan: {plan:?}"
            );
            assert!(
                plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
                "query plan sorts: {plan:?}"
            );
            let ids = statement
                .query_map([1], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(ids, [format!("{:05}", count - 1)]);
            let sorts = statement.get_status(StatementStatus::Sort);
            let steps = statement.get_status(StatementStatus::VmStep);
            measured.push((count, sorts, steps));
            let mut after = conn.prepare(OPEN_JOB_CANDIDATES_AFTER_SQL).unwrap();
            let after_ids = after
                .query_map(params![0, format!("{:05}", count - 1), 1], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(after_ids, [format!("{count:05}")]);
            keyset_measured.push((
                count,
                after.get_status(StatementStatus::Sort),
                after.get_status(StatementStatus::VmStep),
            ));

            conn.execute(
                "UPDATE embedding_jobs SET next_attempt_at=101 WHERE state='pending'",
                [],
            )
            .unwrap();
            let mut deferred = conn.prepare(OPEN_JOB_CANDIDATES_SQL).unwrap();
            let deferred_ids = deferred
                .query_map([1], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(deferred_ids, [format!("{:05}", count - 1)]);
            deferred_measured.push((
                count,
                deferred.get_status(StatementStatus::Sort),
                deferred.get_status(StatementStatus::VmStep),
            ));
        }
        assert!(
            measured
                .iter()
                .all(|(_, sorts, steps)| *sorts == 0 && *steps < 128),
            "due backlog must not be sorted or scanned: {measured:?}"
        );
        assert!(measured[1].2 <= measured[0].2 * 2, "{measured:?}");
        assert!(
            keyset_measured
                .iter()
                .all(|(_, sorts, steps)| *sorts == 0 && *steps < 128),
            "keyset page must not sort or scan the prior prefix: {keyset_measured:?}"
        );
        assert!(
            keyset_measured[1].2 <= keyset_measured[0].2 * 2,
            "{keyset_measured:?}"
        );
        assert!(
            deferred_measured
                .iter()
                .all(|(_, sorts, steps)| *sorts == 0 && *steps < 128),
            "deferred backlog must be bounded by the open-row page: {deferred_measured:?}"
        );
        assert!(
            deferred_measured[1].2 <= deferred_measured[0].2 * 2,
            "{deferred_measured:?}"
        );
    }

    #[test]
    fn eligible_candidate_keyset_is_strict_across_timestamp_ties() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::BASELINE).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(
            "INSERT INTO payloads VALUES ('p',x'61',1,0);
             INSERT INTO vector_generations VALUES ('g','model','fp',8,1,'building',0,0);
             INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                 payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                 source_artifact_digest,created_commit_seq,persisted_at)
             VALUES ('a',x'00','l','messages',1,'text','p','d','normal','s','e','digest',1,0),
                    ('b',x'00','l','messages',1,'text','p','d','normal','s','e','digest',1,0),
                    ('c',x'00','l','messages',1,'text','p','d','normal','s','e','digest',1,0),
                    ('d',x'00','l','messages',1,'text','p','d','normal','s','e','digest',1,0),
                    ('e',x'00','l','messages',1,'text','p','d','normal','s','e','digest',1,0);
             INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,created_at,updated_at)
             VALUES ('a','a','g','pending',1,0),('b','b','g','pending',1,0),
                    ('c','c','g','pending',1,0),('d','d','g','pending',2,0),
                    ('e','e','g','pending',2,0);",
        )
        .unwrap();

        let first = conn
            .prepare(OPEN_JOB_CANDIDATES_SQL)
            .unwrap()
            .query_map([2], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(first, ["a", "b"]);
        let mut after = conn.prepare(OPEN_JOB_CANDIDATES_AFTER_SQL).unwrap();
        let second = after
            .query_map(params![1, "b", 2], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(second, ["c", "d"]);
        let third = after
            .query_map(params![2, "d", 2], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(third, ["e"]);

        let plan = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {OPEN_JOB_CANDIDATES_AFTER_SQL}"
            ))
            .unwrap()
            .query_map(params![1, "b", 2], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|detail| detail.contains("idx_embedding_jobs_open_order")),
            "ordered index missing from plan: {plan:?}"
        );
        assert!(
            plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
            "query plan sorts: {plan:?}"
        );
        assert_eq!(after.get_status(StatementStatus::Sort), 0);
        assert!(after.get_status(StatementStatus::VmStep) < 256);
    }
}
