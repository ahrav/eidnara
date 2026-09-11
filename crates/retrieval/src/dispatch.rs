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

/// The identity query limits the payload query to `limit` rows, avoiding text loads for rows this pass will not take.
pub fn eligible_jobs(
    conn: &GuardedConn<'_>,
    limit: NonZeroUsize,
    now: i64,
) -> Result<Vec<DispatchJob>, ProjectionError> {
    let mut identities = conn.prepare(
        "SELECT job_id FROM embedding_jobs
         WHERE stop_reason IS NULL
           AND ((state='pending' AND (next_attempt_at IS NULL OR next_attempt_at<=?1))
                OR (state='admitted' AND host_job_id IS NOT NULL))
         ORDER BY created_at,job_id LIMIT ?2",
    )?;
    let job_ids: Vec<String> = identities
        .query_map(params![now, limit.get() as i64], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let mut statement = conn.prepare(
        "SELECT j.job_id,j.occurrence_id,o.payload_id,o.source_object_id,o.revision,o.source_artifact_digest,
                CAST(p.bytes AS TEXT),j.state,j.attempts,j.episode_allowance,j.episode_deadline,j.host_job_id,
                g.generation_id,g.embedding_model,g.tokenizer_fingerprint,g.vector_dimension,g.generation_epoch,
                j.episode_id
         FROM embedding_jobs j
         JOIN occurrences o ON o.occurrence_id=j.occurrence_id
         JOIN payloads p ON p.payload_id=o.payload_id
         JOIN vector_generations g ON g.generation_id=j.generation_id
         WHERE j.job_id=?1",
    )?;
    let mut jobs = Vec::with_capacity(job_ids.len());
    for job_id in &job_ids {
        let row = statement
            .query_row([job_id], |row| {
                let epoch: i64 = row.get(16)?;
                let job = DispatchJob {
                    job_id: row.get(0)?,
                    occurrence_id: row.get(1)?,
                    generation: VectorGeneration {
                        generation_id: row.get(12)?,
                        embedding_model: row.get(13)?,
                        tokenizer_fingerprint: row.get(14)?,
                        vector_dimension: row.get(15)?,
                        generation_epoch: epoch.unsigned_abs(),
                    },
                    payload_id: row.get(2)?,
                    source_object_id: row.get(3)?,
                    revision: row.get(4)?,
                    source_artifact_digest: row.get(5)?,
                    text: row.get(6)?,
                    state: row.get(7)?,
                    attempts: row.get(8)?,
                    episode: None,
                    host_job_id: row.get(11)?,
                };
                let episode_id: Option<String> = row.get(17)?;
                let allowance: u32 = row.get(9)?;
                let deadline: Option<i64> = row.get(10)?;
                Ok((job, episode_id, allowance, deadline))
            })
            .optional()?;
        let Some((mut job, episode_id, allowance, deadline)) = row else {
            return Err(ProjectionError::CorruptRow);
        };
        job.episode = Episode::from_columns(episode_id, allowance, deadline)?;
        jobs.push(job);
    }
    Ok(jobs)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// One attempt was charged and the row now names the host job.
    Charged { attempts: u32 },
    /// The same host job is already recorded, so a lost reply was not a lost charge.
    AlreadyCharged,
    /// The episode has no attempt left or its deadline passed; the row is stopped with that reason.
    Stopped(&'static str),
    /// The row is not pending.
    NotPending,
}

/// Charges one attempt for admitting `job_id` to `host` as `host_job_id`.
/// A row without an episode opens one from `grant`; a row with an episode keeps
/// its allowance and deadline. The charge is refused, and the row stopped,
/// when the episode is exhausted or expired.
pub fn charge_admission(
    conn: &GuardedConn<'_>,
    job_id: &str,
    host: &LaneBinding,
    host_job_id: &str,
    grant: EpisodeGrant,
    now: i64,
) -> Result<Admission, ProjectionError> {
    let Some(ledger) = job_ledger(conn, job_id)? else {
        return Ok(Admission::NotPending);
    };
    if ledger.state == "admitted" && ledger.host_job_id.as_deref() == Some(host_job_id) {
        return Ok(Admission::AlreadyCharged);
    }
    if ledger.state != "pending" {
        return Ok(Admission::NotPending);
    }
    let attempts = ledger.attempts;
    let episode = ledger.episode()?;
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
