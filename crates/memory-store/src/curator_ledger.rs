//! The Curator receipt and attempt ledger, and the bounded commit-then-handoff dispatch.
//!
//! A receipt row exists per admitted job from its first claim. The run deadline and execution cutoff are written once at generation 1 and never recomputed; a takeover advances the generation and rebinds the claim but inherits both deadlines, so no recovery path resets a budget. Every write after the first claim is fenced on the generation the caller holds, and completion selects exactly one Kernel result for the current generation in the same transaction that completes the task lease, so a losing generation cannot publish.
//!
//! An attempt row is the marker that precedes any network disclosure. It is inserted only after the claim, module authority, receipt generation, both incarnations, cancellation, cutoff, and allowance checks hold, and the allowance is the count of committed rows across every generation evaluated inside the insert itself, so no fifth attempt can commit. A committed row stays consumed whether or not bytes were sent; a row finished `not_dispatched` proves no disclosure happened.
//!
//! [`MemoryStore::dispatch_curator_attempt`] is the disclosure ordering point: the marker commits, then live time, cancellation, and the claim are rechecked and one synchronous handoff consumes the prepared request while this store still owns its connection. Commit failure hands nothing off; a recheck failure after commit leaves a charged marker and sends nothing. Both stores are released before any network await, and the Kernel guard is taken before this store by the caller.

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::curator_jobs::{
    CuratorJobOutcome, CuratorJobState, finish_curator_job_in_tx, load_curator_job,
};
use crate::task_lease::{
    LeaseAcquireOutcome, LeaseCompleteOutcome, LeaseCompletion, LeaseSelected, TaskLeaseKind,
};
use crate::{
    DurableWriteFamily, MemoryStore, MemoryStoreError, NOTE_EVAL_NO_WORK_RETENTION_MS,
    NOTE_EVAL_RESPONSE_REDACT_MS, NOTE_EVAL_TERMINAL_RETENTION_MS, PreparedWrite, WriteDisposition,
    active_scan_owner_key,
};

/// Run deadline measured from the first claim.
pub const CURATOR_RUN_DEADLINE_MS: i64 = 120_000;
/// Final part of the run reserved for settlement; no attempt starts inside it.
pub const CURATOR_SETTLEMENT_RESERVE_MS: i64 = 10_000;
/// Bound of one attempt; the actual bound is the smaller of this and the time left before the cutoff.
pub const CURATOR_ATTEMPT_MAX_MS: i64 = 30_000;
/// Committed attempts per job across every generation.
pub const CURATOR_MAX_ATTEMPTS: i64 = 4;
pub const CURATOR_MAX_REQUEST_BYTES: u64 = 256 * 1024;
/// One attempt bound: a live worker renews between attempts, and a crashed worker's claim frees inside the run so a takeover can still finish before the cutoff.
pub const CURATOR_TASK_LEASE_MS: i64 = CURATOR_ATTEMPT_MAX_MS;

/// Curator review runs on the shared task-lease ledger, fenced on the `memories` authority like Memory Classifier tasks.
pub const CURATOR_REVIEW_TASK: TaskLeaseKind = TaskLeaseKind {
    task_kind: "curator_review",
    authority_domain: "memories",
    claim_id_prefix: "crc:",
    phases: &["run"],
    lease_ms: CURATOR_TASK_LEASE_MS,
    no_work_retention_ms: NOTE_EVAL_NO_WORK_RETENTION_MS,
    terminal_retention_ms: NOTE_EVAL_TERMINAL_RETENTION_MS,
    response_redact_ms: NOTE_EVAL_RESPONSE_REDACT_MS,
    ledger_cap: 64,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuratorReceiptTerminal {
    Complete,
    Abstained,
    Failed,
    Cancelled,
    Unknown,
    Expired,
}

impl CuratorReceiptTerminal {
    const ALL: [Self; 6] = [
        Self::Complete,
        Self::Abstained,
        Self::Failed,
        Self::Cancelled,
        Self::Unknown,
        Self::Expired,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Abstained => "abstained",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
            Self::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    fn job_outcome(self) -> CuratorJobOutcome {
        match self {
            Self::Complete => CuratorJobOutcome::Completed,
            Self::Abstained => CuratorJobOutcome::Abstained,
            Self::Failed | Self::Cancelled => CuratorJobOutcome::Failed,
            Self::Unknown => CuratorJobOutcome::Unknown,
            Self::Expired => CuratorJobOutcome::Expired,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuratorAttemptTerminal {
    Complete,
    Failed,
    Cancelled,
    Unknown,
    /// The marker committed and no bytes were sent; proof of no disclosure.
    NotDispatched,
}

impl CuratorAttemptTerminal {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
            Self::NotDispatched => "not_dispatched",
        }
    }
}

/// The Kernel result one completion selects; the caller derived the candidate from the job and generation before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSelection {
    pub candidate_id: String,
    pub payload_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorReceipt {
    pub project: String,
    pub causal_identity: String,
    pub database_incarnation_id: String,
    pub kernel_incarnation_id: String,
    pub authority_generation: u64,
    pub generation: u64,
    pub claim_id: String,
    pub run_deadline_ms: i64,
    pub execution_cutoff_ms: i64,
    pub cancelled_at_ms: Option<i64>,
    pub terminal: Option<CuratorReceiptTerminal>,
    pub selected: Option<(u64, ResultSelection)>,
    pub created_at_ms: i64,
}

/// The KTD7 marker tuple a caller binds before disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptMarker {
    pub body_digest: String,
    pub request_bytes: u64,
    pub provider: String,
    pub model: String,
    pub credential_id: String,
    pub policy_union_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorAttempt {
    pub generation: u64,
    pub attempt_index: u32,
    pub marker: AttemptMarker,
    pub attempt_deadline_ms: i64,
    pub committed_at_ms: i64,
    pub terminal: Option<(String, i64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuratorBeginOutcome {
    Begun(CuratorReceipt),
    InProgress(CuratorReceipt),
    Complete(CuratorReceipt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CuratorLedgerRefusal {
    #[error("the request is malformed or over a bound")]
    InvalidRequest,
    #[error("the job is not ready or has passed its queue deadline")]
    JobUnavailable,
    #[error("no receipt exists for the job")]
    Missing,
    #[error("the receipt is not at the caller's generation or is complete")]
    Fenced,
    #[error("the receipt binds another store incarnation or authority generation")]
    BindingMismatch,
    #[error("the claim is unknown, terminal, or expired")]
    ClaimInvalid,
    #[error("the module authority generation changed")]
    AuthorityChanged,
    #[error("the run was cancelled")]
    Cancelled,
    #[error("the execution cutoff has passed")]
    Cutoff,
    #[error("all committed attempts are consumed")]
    AttemptsExhausted,
    #[error("the attempt is unknown or already terminal")]
    AttemptTerminal,
}

#[derive(Debug, thiserror::Error)]
pub enum CuratorLedgerError {
    #[error(transparent)]
    Refused(#[from] CuratorLedgerRefusal),
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
}

/// What happened after the marker committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome<H> {
    /// The handoff consumed the prepared request; `attempt_index` is the charged marker.
    Handed { attempt_index: u32, handoff: H },
    /// The marker committed but the recheck immediately before handoff failed; the attempt is charged and nothing was sent.
    ChargedNotDispatched {
        attempt_index: u32,
        reason: CuratorLedgerRefusal,
    },
}

fn refuse(refusal: CuratorLedgerRefusal) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(refusal))
}

fn refusal_of(error: &rusqlite::Error) -> Option<CuratorLedgerRefusal> {
    match error {
        rusqlite::Error::ToSqlConversionFailure(inner) => {
            inner.downcast_ref::<CuratorLedgerRefusal>().copied()
        }
        _ => None,
    }
}

fn generation_param(generation: u64) -> Result<i64, CuratorLedgerRefusal> {
    i64::try_from(generation).map_err(|_| CuratorLedgerRefusal::InvalidRequest)
}

const RECEIPT_COLUMNS: &str =
    "project, causal_identity, database_incarnation_id, kernel_incarnation_id,
     authority_generation, state, generation, claim_id, run_deadline_ms, execution_cutoff_ms,
     cancelled_at_ms, terminal_kind, selected_generation, selected_candidate_id,
     selected_payload_digest, created_at_ms";

fn receipt_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CuratorReceipt> {
    let invalid = |column: usize, value: String| {
        rusqlite::Error::InvalidColumnType(column, value, rusqlite::types::Type::Text)
    };
    let terminal: Option<String> = row.get(11)?;
    let terminal = terminal
        .map(|kind| CuratorReceiptTerminal::parse(&kind).ok_or_else(|| invalid(11, kind)))
        .transpose()?;
    let selected_generation: Option<i64> = row.get(12)?;
    let candidate_id: Option<String> = row.get(13)?;
    let payload_digest: Option<String> = row.get(14)?;
    let selected = match (selected_generation, candidate_id, payload_digest) {
        (Some(generation), Some(candidate_id), Some(payload_digest)) => Some((
            u64::try_from(generation).map_err(|_| invalid(12, generation.to_string()))?,
            ResultSelection {
                candidate_id,
                payload_digest,
            },
        )),
        (None, None, None) => None,
        _ => return Err(invalid(13, "partial selection".to_string())),
    };
    Ok(CuratorReceipt {
        project: row.get(0)?,
        causal_identity: row.get(1)?,
        database_incarnation_id: row.get(2)?,
        kernel_incarnation_id: row.get(3)?,
        authority_generation: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
        generation: u64::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
        claim_id: row.get(7)?,
        run_deadline_ms: row.get(8)?,
        execution_cutoff_ms: row.get(9)?,
        cancelled_at_ms: row.get(10)?,
        terminal,
        selected,
        created_at_ms: row.get(15)?,
    })
}

fn load_receipt(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
) -> rusqlite::Result<Option<CuratorReceipt>> {
    conn.query_row(
        &format!(
            "SELECT {RECEIPT_COLUMNS} FROM curator_receipts WHERE project = ?1 AND causal_identity = ?2"
        ),
        params![project, causal_identity],
        receipt_from_row,
    )
    .optional()
}

fn store_incarnation(conn: &GuardedConn<'_>) -> rusqlite::Result<String> {
    conn.query_row(
        "SELECT database_incarnation_id FROM curator_store_identity WHERE id = 0",
        [],
        |row| row.get(0),
    )
}

/// The MODULE authority generation of `memories` for the project, as the lease ledger reads it.
fn module_authority_generation(
    conn: &GuardedConn<'_>,
    project: &str,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT generation FROM authority
          WHERE project = ?1 AND domain = ?2 AND state = 'MODULE'
          ORDER BY context_store_uuid LIMIT 1",
        params![project, CURATOR_REVIEW_TASK.authority_domain],
        |row| row.get(0),
    )
    .optional()
}

/// A claim is live when it is this kind's, unfinished, and unexpired at `now_ms`.
fn claim_is_live(
    conn: &GuardedConn<'_>,
    project: &str,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM note_eval_claims
          WHERE project = ?1 AND task_kind = ?2 AND claim_id = ?3
            AND terminal_kind IS NULL AND expires_at > ?4)",
        params![project, CURATOR_REVIEW_TASK.task_kind, claim_id, now_ms],
        |row| row.get(0),
    )
}

/// Writes the receipt at generation 1 for a Ready job, or reports the receipt it already has. The store's own incarnation is read inside the transaction; the caller supplies only the Kernel incarnation it validated.
pub fn begin_curator_receipt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    kernel_incarnation_id: &str,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<CuratorBeginOutcome> {
    if kernel_incarnation_id.len() != 32 || claim_id.is_empty() || claim_id.len() > 200 {
        return Err(refuse(CuratorLedgerRefusal::InvalidRequest));
    }
    let incarnation = store_incarnation(conn)?;
    let authority = module_authority_generation(conn, project)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::AuthorityChanged))?;
    if let Some(existing) = load_receipt(conn, project, causal_identity)? {
        if existing.database_incarnation_id != incarnation
            || existing.kernel_incarnation_id != kernel_incarnation_id
            || i64::try_from(existing.authority_generation).ok() != Some(authority)
        {
            return Err(refuse(CuratorLedgerRefusal::BindingMismatch));
        }
        return Ok(if existing.terminal.is_some() {
            CuratorBeginOutcome::Complete(existing)
        } else {
            CuratorBeginOutcome::InProgress(existing)
        });
    }
    let job = load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::JobUnavailable))?;
    if !matches!(job.state, CuratorJobState::Ready(_)) || job.queue_deadline_ms <= now_ms {
        return Err(refuse(CuratorLedgerRefusal::JobUnavailable));
    }
    if !claim_is_live(conn, project, claim_id, now_ms)? {
        return Err(refuse(CuratorLedgerRefusal::ClaimInvalid));
    }
    let run_deadline_ms = now_ms
        .checked_add(CURATOR_RUN_DEADLINE_MS)
        .ok_or_else(|| refuse(CuratorLedgerRefusal::InvalidRequest))?;
    conn.execute(
        "INSERT INTO curator_receipts (
             project, causal_identity, database_incarnation_id, kernel_incarnation_id,
             authority_generation, state, generation, claim_id, run_deadline_ms,
             execution_cutoff_ms, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'in_progress', 1, ?6, ?7, ?8, ?9, ?9)",
        params![
            project,
            causal_identity,
            incarnation,
            kernel_incarnation_id,
            authority,
            claim_id,
            run_deadline_ms,
            run_deadline_ms - CURATOR_SETTLEMENT_RESERVE_MS,
            now_ms,
        ],
    )?;
    load_receipt(conn, project, causal_identity)?
        .map(CuratorBeginOutcome::Begun)
        .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))
}

/// Adopts an in-progress receipt at `predecessor_generation` under the next generation with a new live claim. Deadlines are not in the update, so they are inherited unchanged; a takeover at or past the cutoff succeeds but can start no attempt.
pub fn take_over_curator_receipt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    predecessor_generation: u64,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<CuratorReceipt> {
    let predecessor = generation_param(predecessor_generation).map_err(refuse)?;
    if !claim_is_live(conn, project, claim_id, now_ms)? {
        return Err(refuse(CuratorLedgerRefusal::ClaimInvalid));
    }
    let changed = conn.execute(
        "UPDATE curator_receipts
            SET generation = generation + 1, claim_id = ?4, updated_at_ms = ?5
          WHERE project = ?1 AND causal_identity = ?2 AND state = 'in_progress' AND generation = ?3",
        params![project, causal_identity, predecessor, claim_id, now_ms],
    )?;
    if changed == 0 {
        return Err(refuse(CuratorLedgerRefusal::Fenced));
    }
    load_receipt(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))
}

/// Every check KTD7 names, then the marker insert whose `WHERE` counts the committed attempts across all generations. Returns the attempt index and its absolute deadline.
#[allow(clippy::too_many_arguments)]
pub fn commit_curator_attempt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    generation: u64,
    claim_id: &str,
    kernel_incarnation_id: &str,
    marker: &AttemptMarker,
    now_ms: i64,
) -> rusqlite::Result<CuratorAttempt> {
    let generation = generation_param(generation).map_err(refuse)?;
    if marker.body_digest.len() != 64
        || marker.policy_union_digest.len() != 64
        || marker.request_bytes == 0
        || marker.request_bytes > CURATOR_MAX_REQUEST_BYTES
        || marker.provider.is_empty()
        || marker.model.is_empty()
        || marker.credential_id.is_empty()
    {
        return Err(refuse(CuratorLedgerRefusal::InvalidRequest));
    }
    let receipt = load_receipt(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))?;
    if receipt.terminal.is_some() || i64::try_from(receipt.generation).ok() != Some(generation) {
        return Err(refuse(CuratorLedgerRefusal::Fenced));
    }
    if receipt.claim_id != claim_id || !claim_is_live(conn, project, claim_id, now_ms)? {
        return Err(refuse(CuratorLedgerRefusal::ClaimInvalid));
    }
    if receipt.database_incarnation_id != store_incarnation(conn)?
        || receipt.kernel_incarnation_id != kernel_incarnation_id
    {
        return Err(refuse(CuratorLedgerRefusal::BindingMismatch));
    }
    if module_authority_generation(conn, project)?
        != i64::try_from(receipt.authority_generation).ok()
    {
        return Err(refuse(CuratorLedgerRefusal::AuthorityChanged));
    }
    if receipt.cancelled_at_ms.is_some() {
        return Err(refuse(CuratorLedgerRefusal::Cancelled));
    }
    if now_ms >= receipt.execution_cutoff_ms {
        return Err(refuse(CuratorLedgerRefusal::Cutoff));
    }
    let attempt_deadline_ms = (now_ms + CURATOR_ATTEMPT_MAX_MS).min(receipt.execution_cutoff_ms);
    let request_bytes = i64::try_from(marker.request_bytes)
        .map_err(|_| refuse(CuratorLedgerRefusal::InvalidRequest))?;
    let inserted = conn.execute(
        "INSERT INTO curator_attempts (
             project, causal_identity, generation, attempt_index, body_digest, request_bytes,
             provider, model, credential_id, policy_union_digest, attempt_deadline_ms, committed_at_ms
         )
         SELECT ?1, ?2, ?3, COALESCE(MAX(attempt_index) + 1, 0), ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
           FROM curator_attempts WHERE project = ?1 AND causal_identity = ?2 AND generation = ?3
          HAVING (SELECT COUNT(*) FROM curator_attempts
                   WHERE project = ?1 AND causal_identity = ?2) < ?12",
        params![
            project,
            causal_identity,
            generation,
            marker.body_digest,
            request_bytes,
            marker.provider,
            marker.model,
            marker.credential_id,
            marker.policy_union_digest,
            attempt_deadline_ms,
            now_ms,
            CURATOR_MAX_ATTEMPTS,
        ],
    )?;
    if inserted == 0 {
        return Err(refuse(CuratorLedgerRefusal::AttemptsExhausted));
    }
    let attempt_index: i64 = conn.query_row(
        "SELECT MAX(attempt_index) FROM curator_attempts
          WHERE project = ?1 AND causal_identity = ?2 AND generation = ?3",
        params![project, causal_identity, generation],
        |row| row.get(0),
    )?;
    Ok(CuratorAttempt {
        generation: receipt.generation,
        attempt_index: u32::try_from(attempt_index).unwrap_or(u32::MAX),
        marker: marker.clone(),
        attempt_deadline_ms,
        committed_at_ms: now_ms,
        terminal: None,
    })
}

pub fn finish_curator_attempt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    generation: u64,
    attempt_index: u32,
    terminal: CuratorAttemptTerminal,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let changed = conn.execute(
        "UPDATE curator_attempts SET terminal_kind = ?5, terminal_at_ms = ?6
          WHERE project = ?1 AND causal_identity = ?2 AND generation = ?3 AND attempt_index = ?4
            AND terminal_kind IS NULL",
        params![
            project,
            causal_identity,
            generation_param(generation).map_err(refuse)?,
            i64::from(attempt_index),
            terminal.as_str(),
            now_ms
        ],
    )?;
    if changed == 0 {
        return Err(refuse(CuratorLedgerRefusal::AttemptTerminal));
    }
    Ok(())
}

/// Committed attempts under the job across every generation, oldest first.
pub fn list_curator_attempts_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
) -> rusqlite::Result<Vec<CuratorAttempt>> {
    let mut statement = conn.prepare_cached(
        "SELECT generation, attempt_index, body_digest, request_bytes, provider, model,
                credential_id, policy_union_digest, attempt_deadline_ms, committed_at_ms,
                terminal_kind, terminal_at_ms
           FROM curator_attempts WHERE project = ?1 AND causal_identity = ?2
          ORDER BY generation, attempt_index",
    )?;
    let rows = statement.query_map(params![project, causal_identity], |row| {
        let terminal_kind: Option<String> = row.get(10)?;
        let terminal_at: Option<i64> = row.get(11)?;
        Ok(CuratorAttempt {
            generation: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
            attempt_index: u32::try_from(row.get::<_, i64>(1)?).unwrap_or(u32::MAX),
            marker: AttemptMarker {
                body_digest: row.get(2)?,
                request_bytes: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                provider: row.get(4)?,
                model: row.get(5)?,
                credential_id: row.get(6)?,
                policy_union_digest: row.get(7)?,
            },
            attempt_deadline_ms: row.get(8)?,
            committed_at_ms: row.get(9)?,
            terminal: terminal_kind.zip(terminal_at),
        })
    })?;
    rows.collect()
}

fn ledger_write(project: &str, operation: &str, causal_identity: &str) -> PreparedWrite {
    let mut write = PreparedWrite::new(DurableWriteFamily::CuratorJobs);
    write.domain_owner(
        "project",
        project,
        active_scan_owner_key(&["curator-ledger", operation, causal_identity]),
    );
    write
}

impl MemoryStore {
    fn ledger_transaction<T>(
        &self,
        project: &str,
        operation: &str,
        causal_identity: &str,
        body: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<WriteDisposition<T>>,
    ) -> Result<T, CuratorLedgerError> {
        let refusal = std::cell::Cell::new(None);
        let result =
            ledger_write(project, operation, causal_identity).execute(&self.inner, |coordinated| {
                body(coordinated.tx()).inspect_err(|error| refusal.set(refusal_of(error)))
            });
        match (result, refusal.get()) {
            (Err(_), Some(refusal)) => Err(CuratorLedgerError::Refused(refusal)),
            (Err(error), None) => Err(CuratorLedgerError::Store(error)),
            (Ok(value), _) => Ok(value),
        }
    }

    /// Leases one Ready job for a worker slot through the shared task-lease ledger; the claim's task id is the job row's rowid.
    #[allow(clippy::too_many_arguments)]
    pub fn acquire_curator_task(
        &self,
        project: &str,
        acquisition_id: &str,
        worker_instance: &str,
        slot: i64,
        registration_generation: i64,
        causal_identity: &str,
        now_ms: i64,
    ) -> Result<LeaseAcquireOutcome<String>, MemoryStoreError> {
        let identity = causal_identity.to_string();
        self.acquire_task_lease(
            &CURATOR_REVIEW_TASK,
            project,
            acquisition_id,
            worker_instance,
            slot,
            registration_generation,
            now_ms,
            |tx| {
                let row: Option<i64> = tx
                    .query_row(
                        "SELECT rowid FROM curator_jobs
                          WHERE project = ?1 AND causal_identity = ?2 AND state = 'ready'
                            AND queue_deadline_ms > ?3
                            AND NOT EXISTS(SELECT 1 FROM note_eval_claims c
                                            WHERE c.project = ?1 AND c.task_kind = ?4
                                              AND c.note_id = curator_jobs.rowid
                                              AND c.terminal_kind IS NULL)",
                        params![project, identity, now_ms, CURATOR_REVIEW_TASK.task_kind],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(match row {
                    Some(rowid) => LeaseSelected::Claim {
                        note_id: rowid,
                        phase: "run".to_string(),
                        task: identity.clone(),
                        source_revision: 0,
                        state_version: 0,
                        policy_version: 0,
                    },
                    None => LeaseSelected::NoWork {
                        cycle_exhausted: false,
                    },
                })
            },
            |tx, rowid| {
                tx.query_row(
                    "SELECT causal_identity FROM curator_jobs WHERE rowid = ?1",
                    [rowid],
                    |row| row.get(0),
                )
                .optional()
            },
        )
    }

    /// Renews a live claim; the receipt's deadlines are unaffected.
    pub fn renew_curator_task(
        &self,
        project: &str,
        claim_id: &str,
        worker_instance: &str,
        slot: i64,
        registration_generation: i64,
        now_ms: i64,
    ) -> Result<crate::task_lease::LeaseRenewOutcome, MemoryStoreError> {
        self.renew_task_lease(
            &CURATOR_REVIEW_TASK,
            project,
            claim_id,
            worker_instance,
            slot,
            registration_generation,
            now_ms,
        )
    }

    pub fn begin_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
        kernel_incarnation_id: &str,
        claim_id: &str,
        now_ms: i64,
    ) -> Result<CuratorBeginOutcome, CuratorLedgerError> {
        self.ledger_transaction(project, "begin", causal_identity, |conn| {
            let outcome = begin_curator_receipt_in_tx(
                conn,
                project,
                causal_identity,
                kernel_incarnation_id,
                claim_id,
                now_ms,
            )?;
            Ok(match outcome {
                CuratorBeginOutcome::Begun(_) => WriteDisposition::Applied(outcome),
                _ => WriteDisposition::Replay(outcome),
            })
        })
    }

    pub fn take_over_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
        predecessor_generation: u64,
        claim_id: &str,
        now_ms: i64,
    ) -> Result<CuratorReceipt, CuratorLedgerError> {
        self.ledger_transaction(project, "take-over", causal_identity, |conn| {
            take_over_curator_receipt_in_tx(
                conn,
                project,
                causal_identity,
                predecessor_generation,
                claim_id,
                now_ms,
            )
            .map(WriteDisposition::Applied)
        })
    }

    /// Records the cancellation request; no later attempt commits and the next handoff recheck withholds.
    pub fn cancel_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
        now_ms: i64,
    ) -> Result<(), CuratorLedgerError> {
        self.ledger_transaction(project, "cancel", causal_identity, |conn| {
            let changed = conn.execute(
                "UPDATE curator_receipts SET cancelled_at_ms = COALESCE(cancelled_at_ms, ?3), updated_at_ms = ?3
                  WHERE project = ?1 AND causal_identity = ?2 AND state = 'in_progress'",
                params![project, causal_identity, now_ms],
            )?;
            if changed == 0 {
                return Err(refuse(CuratorLedgerRefusal::Fenced));
            }
            Ok(WriteDisposition::Applied(()))
        })
    }

    /// Commits the marker and, in the same connection ownership, hands the prepared request off once. `now` is read again after the commit: a claim, attempt deadline, cutoff, or cancellation that lapsed in between leaves the charged marker and sends nothing. The caller has already taken the Kernel guard and must not call either store from `handoff`.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_curator_attempt<P, H>(
        &self,
        project: &str,
        causal_identity: &str,
        generation: u64,
        claim_id: &str,
        kernel_incarnation_id: &str,
        marker: &AttemptMarker,
        prepared: P,
        now: impl Fn() -> i64,
        handoff: impl FnOnce(P) -> H,
    ) -> Result<DispatchOutcome<H>, CuratorLedgerError> {
        let refusal = std::cell::Cell::new(None);
        let result = self.inner.with_conn_fenced_then_handoff(
            |conn| {
                commit_curator_attempt_in_tx(
                    conn,
                    project,
                    causal_identity,
                    generation,
                    claim_id,
                    kernel_incarnation_id,
                    marker,
                    now(),
                )
                .map(|attempt| (attempt.clone(), Some((attempt, prepared))))
                .inspect_err(|error| refusal.set(refusal_of(error)))
            },
            |conn, (attempt, prepared)| {
                let live = now();
                let receipt = load_receipt(conn, project, causal_identity)?
                    .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))?;
                let recheck = if !claim_is_live(conn, project, claim_id, live)? {
                    Some(CuratorLedgerRefusal::ClaimInvalid)
                } else if receipt.cancelled_at_ms.is_some() {
                    Some(CuratorLedgerRefusal::Cancelled)
                } else if live >= attempt.attempt_deadline_ms || live >= receipt.execution_cutoff_ms
                {
                    Some(CuratorLedgerRefusal::Cutoff)
                } else {
                    None
                };
                Ok(match recheck {
                    Some(reason) => Err(reason),
                    None => Ok(handoff(prepared)),
                })
            },
        );
        match (result, refusal.get()) {
            (Err(_), Some(refusal)) => Err(CuratorLedgerError::Refused(refusal)),
            (Err(error), None) => Err(CuratorLedgerError::Store(error.into())),
            (Ok((attempt, Some(Ok(handoff)))), _) => Ok(DispatchOutcome::Handed {
                attempt_index: attempt.attempt_index,
                handoff,
            }),
            (Ok((attempt, Some(Err(reason)))), _) => Ok(DispatchOutcome::ChargedNotDispatched {
                attempt_index: attempt.attempt_index,
                reason,
            }),
            (Ok((_, None)), _) => Err(CuratorLedgerError::Store(MemoryStoreError::Serde(
                "the marker committed without a prepared request".to_string(),
            ))),
        }
    }

    pub fn finish_curator_attempt(
        &self,
        project: &str,
        causal_identity: &str,
        generation: u64,
        attempt_index: u32,
        terminal: CuratorAttemptTerminal,
        now_ms: i64,
    ) -> Result<(), CuratorLedgerError> {
        self.ledger_transaction(project, "finish-attempt", causal_identity, |conn| {
            finish_curator_attempt_in_tx(
                conn,
                project,
                causal_identity,
                generation,
                attempt_index,
                terminal,
                now_ms,
            )
            .map(WriteDisposition::Applied)
        })
    }

    /// Completes the task lease and the receipt in one fenced transaction. The receipt must be in progress at `generation` under `claim_id`; a `Complete` terminal selects exactly one Kernel result for that generation, and the job row records the matching outcome. A predecessor generation or a stale claim writes nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
        claim_id: &str,
        completion_id: &str,
        worker_instance: &str,
        slot: i64,
        generation: u64,
        terminal: CuratorReceiptTerminal,
        selection: Option<&ResultSelection>,
        now_ms: i64,
    ) -> Result<LeaseCompleteOutcome, MemoryStoreError> {
        if (terminal == CuratorReceiptTerminal::Complete) != selection.is_some()
            || selection.is_some_and(|selection| {
                selection.payload_digest.len() != 64
                    || selection.candidate_id.is_empty()
                    || selection.candidate_id.len() > 256
            })
        {
            return Ok(LeaseCompleteOutcome::Conflict { kind: "invalid" });
        }
        let generation = generation_param(generation).map_err(|_| {
            MemoryStoreError::Serde("generation exceeds the storable range".to_string())
        })?;
        // A caller holding another generation or another claim is fenced before the lease is touched, so a stale worker's completion ends nothing.
        let fenced = self
            .lookup_curator_receipt(project, causal_identity)?
            .is_none_or(|receipt| {
                receipt.terminal.is_none()
                    && (i64::try_from(receipt.generation).ok() != Some(generation)
                        || receipt.claim_id != claim_id)
            });
        if fenced {
            return Ok(LeaseCompleteOutcome::Conflict { kind: "fenced" });
        }
        self.complete_task_lease(
            &CURATOR_REVIEW_TASK,
            project,
            claim_id,
            completion_id,
            worker_instance,
            slot,
            now_ms,
            |coordinated, _claim| {
                let tx = coordinated.tx;
                let changed = tx.execute(
                    "UPDATE curator_receipts
                        SET state = 'complete', terminal_kind = ?5, selected_generation = ?6,
                            selected_candidate_id = ?7, selected_payload_digest = ?8, updated_at_ms = ?9
                      WHERE project = ?1 AND causal_identity = ?2 AND state = 'in_progress'
                        AND generation = ?3 AND claim_id = ?4",
                    params![
                        project,
                        causal_identity,
                        generation,
                        claim_id,
                        terminal.as_str(),
                        selection.map(|_| generation),
                        selection.map(|selection| selection.candidate_id.as_str()),
                        selection.map(|selection| selection.payload_digest.as_str()),
                        now_ms,
                    ],
                )?;
                if changed == 0 {
                    return Ok(LeaseCompletion::Stale);
                }
                finish_curator_job_in_tx(tx, project, causal_identity, terminal.job_outcome(), now_ms)?;
                Ok(LeaseCompletion::Applied {
                    response_json: format!("{{\"terminal\":\"{}\"}}", terminal.as_str()),
                })
            },
        )
    }

    pub fn lookup_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
    ) -> Result<Option<CuratorReceipt>, MemoryStoreError> {
        self.inner
            .with_conn(|conn| load_receipt(conn, project, causal_identity))
            .map_err(Into::into)
    }

    pub fn list_curator_attempts(
        &self,
        project: &str,
        causal_identity: &str,
    ) -> Result<Vec<CuratorAttempt>, MemoryStoreError> {
        self.inner
            .with_conn(|conn| list_curator_attempts_in_tx(conn, project, causal_identity))
            .map_err(Into::into)
    }
}
