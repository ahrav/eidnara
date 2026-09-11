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

/// A job row the dispatcher may act on, with the input it embeds and the
/// current-input identity the completion is guarded by.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The episode the row runs under, or the one its first admission opens.
    pub episode: String,
    pub attempts: u32,
    pub episode_allowance: u32,
    pub episode_deadline: Option<i64>,
    pub host_job_id: Option<String>,
}

impl DispatchJob {
    /// Why the row's episode admits no further attempt at `now`, or `None` when it does. A row without an episode is judged against the grant that would open one.
    pub fn episode_refusal(&self, grant: EpisodeGrant, now: i64) -> Option<&'static str> {
        refusal(
            self.attempts,
            self.episode_allowance,
            self.episode_deadline,
            grant,
            now,
        )
    }
}

fn refusal(
    attempts: u32,
    allowance: u32,
    deadline: Option<i64>,
    grant: EpisodeGrant,
    now: i64,
) -> Option<&'static str> {
    let (allowance, deadline) = match deadline {
        Some(deadline) => (allowance, deadline),
        None => (grant.allowance.get(), grant.deadline),
    };
    if attempts >= allowance {
        Some(EXHAUSTED)
    } else if now > deadline {
        Some(DEADLINE_EXPIRED)
    } else {
        None
    }
}

/// The pending rows whose retry time has come and the admitted rows still
/// held by a host, oldest job identifier first. Stopped rows never appear.
pub fn eligible_jobs(
    conn: &GuardedConn<'_>,
    limit: NonZeroUsize,
    now: i64,
) -> Result<Vec<DispatchJob>, ProjectionError> {
    let mut statement = conn.prepare(
        "SELECT j.job_id,j.occurrence_id,o.payload_id,o.source_object_id,o.revision,o.source_artifact_digest,
                CAST(p.bytes AS TEXT),j.state,j.attempts,j.episode_allowance,j.episode_deadline,j.host_job_id,
                g.generation_id,g.embedding_model,g.tokenizer_fingerprint,g.vector_dimension,g.generation_epoch,
                COALESCE(j.episode_id,j.job_id||'/1')
         FROM embedding_jobs j
         JOIN occurrences o ON o.occurrence_id=j.occurrence_id
         JOIN payloads p ON p.payload_id=o.payload_id
         JOIN vector_generations g ON g.generation_id=j.generation_id
         WHERE j.stop_reason IS NULL
           AND ((j.state='pending' AND (j.next_attempt_at IS NULL OR j.next_attempt_at<=?1))
                OR (j.state='admitted' AND j.host_job_id IS NOT NULL))
         ORDER BY j.job_id LIMIT ?2",
    )?;
    let rows = statement.query_map(params![now, limit.get() as i64], |row| {
        let epoch: i64 = row.get(16)?;
        Ok(DispatchJob {
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
            episode: row.get(17)?,
            attempts: row.get(8)?,
            episode_allowance: row.get(9)?,
            episode_deadline: row.get(10)?,
            host_job_id: row.get(11)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
    if let Some(reason) = refusal(
        attempts,
        ledger.episode_allowance,
        ledger.episode_deadline,
        grant,
        now,
    ) {
        stop_job(conn, job_id, reason, now)?;
        return Ok(Admission::Stopped(reason));
    }
    let (episode_id, allowance, deadline) = match ledger.episode_id {
        Some(id) => (id, ledger.episode_allowance, ledger.episode_deadline),
        None => (
            format!("{job_id}/1"),
            grant.allowance.get(),
            Some(grant.deadline),
        ),
    };
    let epoch = i64::try_from(host.table_epoch).map_err(|_| ProjectionError::CorruptRow)?;
    conn.execute(
        "UPDATE embedding_jobs SET state='admitted',attempts=attempts+1,host_job_id=?2,host_incarnation=?3,admitted_epoch=?4,
                episode_id=?5,episode_allowance=?6,episode_deadline=?7,next_attempt_at=NULL,updated_at=?8
         WHERE job_id=?1 AND state='pending'",
        params![job_id, host_job_id, host.host_incarnation, epoch, episode_id, allowance, deadline, now],
    )?;
    Ok(Admission::Charged {
        attempts: attempts + 1,
    })
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
    let (attempts, allowance) = (ledger.attempts, ledger.episode_allowance);
    // A row that never opened an episode has no allowance to exhaust yet.
    if allowance > 0 && attempts >= allowance {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// A new episode is open under `authorization_ref`.
    Granted { episode_id: String },
    /// The same authorization already opened its episode; nothing changed.
    Replayed,
    /// The row is not stopped.
    NotStopped,
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
    let Some(ledger) = job_ledger(conn, job_id)? else {
        return Ok(Recovery::NotStopped);
    };
    if ledger.authorization_ref.as_deref() == Some(authorization_ref) {
        return Ok(Recovery::Replayed);
    }
    if ledger.state != "failed" {
        return Ok(Recovery::NotStopped);
    }
    let episode_id = format!("{job_id}/{authorization_ref}");
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
