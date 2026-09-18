//! The Curator receipt and attempt ledger, and the bounded commit-then-handoff dispatch.
//!
//! A receipt row exists per admitted job from its first claim. The run deadline and execution cutoff are written once at generation 1 and never recomputed; a takeover advances the generation and rebinds the claim but inherits both deadlines, so no recovery path resets a budget. Every write after the first claim is fenced on the generation the caller holds, and completion selects exactly one Kernel result for the current generation in the same transaction that completes the task lease, so a losing generation cannot publish.
//!
//! An attempt row is the marker that precedes any network disclosure. It is inserted only after the claim, module authority, receipt generation, both incarnations, cancellation, cutoff, and allowance checks hold, and the allowance is the count of committed rows across every generation evaluated inside the insert itself, so no fifth attempt can commit. A committed row stays consumed whether or not bytes were sent; a row finished `not_dispatched` proves no disclosure happened.
//!
//! [`MemoryStore::dispatch_curator_attempt`] is the disclosure ordering point: the marker commits, then live time, cancellation, and the claim are rechecked and one synchronous handoff consumes the prepared request while this store still owns its connection. Commit failure hands nothing off; a recheck failure after commit leaves a charged marker and sends nothing. Both stores are released before any network await, and the Kernel guard is taken before this store by the caller.

use context_core::canonical_json::is_lower_hex;
use rusqlite::{OptionalExtension, params};
use storage::{GuardedConn, HandoffOutcome};

use crate::curator_jobs::{
    CuratorJobOutcome, CuratorJobState, ReviewTarget, check_project, curator_write,
    finish_curator_job_in_tx, load_curator_job, store_incarnation_in_tx,
};
use crate::task_lease::{
    LeaseAcquireOutcome, LeaseCompleteOutcome, LeaseCompletion, LeaseSelected, TaskLeaseKind,
    redaction_error,
};
use crate::{
    MemoryStore, MemoryStoreError, NOTE_EVAL_NO_WORK_RETENTION_MS, NOTE_EVAL_RESPONSE_REDACT_MS,
    NOTE_EVAL_TERMINAL_RETENTION_MS, WriteDisposition, active_scan_owner_key,
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
/// Byte bounds of the marker's caller text. The schema bounds the same columns in characters; these keep every permanent marker inside the receipt charge whatever the encoding.
pub const MAX_PROVIDER_BYTES: usize = 128;
pub const MAX_MODEL_BYTES: usize = 256;
pub const MAX_CREDENTIAL_ID_BYTES: usize = 256;
/// The authority row's context store identity is copied onto every receipt; one the receipt charge cannot hold is not a bindable authority.
pub const MAX_AUTHORITY_CONTEXT_STORE_BYTES: usize = 256;
/// One attempt bound plus the settlement reserve, so an attempt that runs to its bound still has a live claim to record its result under, while a crashed worker's claim frees inside the run so a takeover can still finish before the cutoff. A live worker renews between attempts.
pub const CURATOR_TASK_LEASE_MS: i64 = CURATOR_ATTEMPT_MAX_MS + CURATOR_SETTLEMENT_RESERVE_MS;

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
    const ALL: [Self; 5] = [
        Self::Complete,
        Self::Failed,
        Self::Cancelled,
        Self::Unknown,
        Self::NotDispatched,
    ];

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

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

/// Why a review abstained, on the operator surface (Q28). `owner_sensitive`, `wrong_scope`, and `secret` are the Kernel's egress denial codes a revalidation can surface; the rest name settlement conditions under which no content may publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbstainReason {
    /// A disclosed input's served class bars the run's destination.
    OwnerSensitive,
    /// A disclosed input left the project's scope.
    WrongScope,
    /// The model text carries a detected secret or redaction placeholder.
    Secret,
    /// A disclosed input no longer resolves as it did when rendered.
    ExpectationChanged,
    /// The proposal cites evidence the run never disclosed.
    UndisclosedCitation,
    /// A capacity refusal followed a disclosure, so the model reasoned over a truncated evidence set.
    PartialDisclosure,
    /// The model declined to conclude.
    ModelDeclined,
    /// The proposal breaks a payload rule the Kernel enforces at staging: a field shape, a bound, or an action that does not fit its target and text.
    InvalidProposal,
}

impl AbstainReason {
    pub const ALL: [Self; 8] = [
        Self::OwnerSensitive,
        Self::WrongScope,
        Self::Secret,
        Self::ExpectationChanged,
        Self::UndisclosedCitation,
        Self::PartialDisclosure,
        Self::ModelDeclined,
        Self::InvalidProposal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnerSensitive => "owner_sensitive",
            Self::WrongScope => "wrong_scope",
            Self::Secret => "secret",
            Self::ExpectationChanged => "expectation_changed",
            Self::UndisclosedCitation => "undisclosed_citation",
            Self::PartialDisclosure => "partial_disclosure",
            Self::ModelDeclined => "model_declined",
            Self::InvalidProposal => "invalid_proposal",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

/// How one receipt completes, as the worker reports it. `Complete` is the only completion that selects a Kernel result; `Abstained` is the only one that carries a reason. `Cancelled` and `Expired` are derived from the receipt itself at completion and refused when supplied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptCompletion {
    Complete(ResultSelection),
    Abstained(AbstainReason),
    Failed,
    Cancelled,
    Unknown,
    Expired,
}

impl ReceiptCompletion {
    pub fn terminal(&self) -> CuratorReceiptTerminal {
        match self {
            Self::Complete(_) => CuratorReceiptTerminal::Complete,
            Self::Abstained(_) => CuratorReceiptTerminal::Abstained,
            Self::Failed => CuratorReceiptTerminal::Failed,
            Self::Cancelled => CuratorReceiptTerminal::Cancelled,
            Self::Unknown => CuratorReceiptTerminal::Unknown,
            Self::Expired => CuratorReceiptTerminal::Expired,
        }
    }

    fn selection(&self) -> Option<&ResultSelection> {
        match self {
            Self::Complete(selection) => Some(selection),
            _ => None,
        }
    }

    fn abstained_reason(&self) -> Option<AbstainReason> {
        match self {
            Self::Abstained(reason) => Some(*reason),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorReceipt {
    pub project: String,
    pub causal_identity: String,
    pub database_incarnation_id: String,
    pub kernel_incarnation_id: String,
    /// The authority row the run is bound to: the context store that owned `memories` at begin, and its generation then.
    pub authority_context_store: String,
    pub authority_generation: u64,
    pub generation: u64,
    pub claim_id: String,
    pub run_deadline_ms: i64,
    pub execution_cutoff_ms: i64,
    pub cancelled_at_ms: Option<i64>,
    pub terminal: Option<CuratorReceiptTerminal>,
    pub abstained_reason: Option<AbstainReason>,
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
    pub terminal: Option<(CuratorAttemptTerminal, i64)>,
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
    /// The caller's clock reads earlier than the newest committed marker, so the attempt allowance is untouched and a retry after the clock catches up can succeed.
    #[error("the caller's clock is behind the newest committed attempt")]
    ClockBehind,
    #[error("the attempt is unknown or already terminal")]
    AttemptTerminal,
    #[error("the job is not ready or has passed its queue deadline")]
    JobDeadline,
    #[error("the post-commit recheck could not read the ledger")]
    RecheckUnavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum CuratorLedgerError {
    #[error(transparent)]
    Refused(#[from] CuratorLedgerRefusal),
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
}

/// What happened after the marker committed.
#[derive(Debug)]
pub enum DispatchOutcome<H> {
    /// The handoff consumed the prepared request. `attempt_deadline_ms` is the absolute bound the marker committed under; the caller bounds its network wait by it. `release` reports whether the store's read-only view was restored afterwards; a failure there cannot recall the handoff, but it means this store's connection may refuse later writes, so it is carried here rather than dropped.
    Handed {
        attempt_index: u32,
        attempt_deadline_ms: i64,
        handoff: H,
        release: Result<(), MemoryStoreError>,
    },
    /// The recheck immediately before handoff failed; nothing was sent and the attempt is charged. It is finished `not_dispatched` when `finished` is true; otherwise it stays unterminated and counts as unknown.
    ChargedNotDispatched {
        attempt_index: u32,
        reason: CuratorLedgerRefusal,
        finished: bool,
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

/// Completed receipts one list call returns at most.
pub const MAX_RECEIPT_PAGE: usize = 64;

const RECEIPT_COLUMNS: &str =
    "project, causal_identity, database_incarnation_id, kernel_incarnation_id,
     authority_generation, state, generation, claim_id, run_deadline_ms, execution_cutoff_ms,
     cancelled_at_ms, terminal_kind, selected_generation, selected_candidate_id,
     selected_payload_digest, created_at_ms, authority_context_store, abstained_reason";

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
    let abstained_reason: Option<String> = row.get(17)?;
    let abstained_reason = abstained_reason
        .map(|reason| AbstainReason::parse(&reason).ok_or_else(|| invalid(17, reason)))
        .transpose()?;
    if (terminal == Some(CuratorReceiptTerminal::Abstained)) != abstained_reason.is_some() {
        return Err(invalid(17, "abstention without its reason".to_string()));
    }
    Ok(CuratorReceipt {
        project: row.get(0)?,
        causal_identity: row.get(1)?,
        database_incarnation_id: row.get(2)?,
        kernel_incarnation_id: row.get(3)?,
        authority_context_store: row.get(16)?,
        authority_generation: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
        generation: u64::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
        claim_id: row.get(7)?,
        run_deadline_ms: row.get(8)?,
        execution_cutoff_ms: row.get(9)?,
        cancelled_at_ms: row.get(10)?,
        terminal,
        abstained_reason,
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

/// The live claim's expiry. The `note_id` match binds the claim to this job's row, so a claim a worker holds on another job cannot drive this job's receipt.
fn live_claim_expiry(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT expires_at FROM note_eval_claims
          WHERE project = ?1 AND task_kind = ?2 AND claim_id = ?3
            AND terminal_kind IS NULL AND expires_at > ?4
            AND note_id = (SELECT job_id FROM curator_jobs
                            WHERE project = ?1 AND causal_identity = ?5)",
        params![
            project,
            CURATOR_REVIEW_TASK.task_kind,
            claim_id,
            now_ms,
            causal_identity
        ],
        |row| row.get(0),
    )
    .optional()
}

fn claim_is_live(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<bool> {
    Ok(live_claim_expiry(conn, project, causal_identity, claim_id, now_ms)?.is_some())
}

/// The `memories` authority row the ledger binds to: the owning context store and its generation. Rows are versioned per context store, so the pair, not the generation alone, identifies an authority.
fn current_authority(
    conn: &GuardedConn<'_>,
    project: &str,
) -> rusqlite::Result<Option<(String, i64)>> {
    conn.query_row(
        "SELECT context_store_uuid, generation FROM authority
          WHERE project = ?1 AND domain = ?2 AND state = 'MODULE'
          ORDER BY context_store_uuid
          LIMIT 1",
        params![project, CURATOR_REVIEW_TASK.authority_domain],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

fn authority_matches(receipt: &CuratorReceipt, current: Option<(String, i64)>) -> bool {
    current.is_some_and(|(context_store, generation)| {
        context_store == receipt.authority_context_store
            && i64::try_from(receipt.authority_generation).ok() == Some(generation)
    })
}

/// The newest instant the ledger already holds for the job: the job row's own timestamps, every marker commit and terminal, the receipt's creation and latest takeover, and every claim's creation and terminal. A write dated before it is a clock that stepped back and is refused as `ClockBehind` rather than judged against a time the ledger has already passed.
fn ledger_clock_floor(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT MAX(
             COALESCE((SELECT MAX(created_at_ms, updated_at_ms) FROM curator_jobs
                        WHERE project = ?1 AND causal_identity = ?2), 0),
             COALESCE((SELECT MAX(MAX(committed_at_ms), COALESCE(MAX(terminal_at_ms), 0))
                         FROM curator_attempts WHERE project = ?1 AND causal_identity = ?2), 0),
             COALESCE((SELECT MAX(created_at_ms, updated_at_ms) FROM curator_receipts
                        WHERE project = ?1 AND causal_identity = ?2), 0),
             COALESCE((SELECT MAX(MAX(created_at_ms), COALESCE(MAX(terminal_at_ms), 0))
                         FROM note_eval_claims
                        WHERE project = ?1 AND task_kind = ?3
                          AND note_id = (SELECT job_id FROM curator_jobs
                                          WHERE project = ?1 AND causal_identity = ?2)), 0))",
        params![project, causal_identity, CURATOR_REVIEW_TASK.task_kind],
        |row| row.get(0),
    )
}

/// The job's queue deadline while it is still Ready and inside that deadline; `None` once it is not open for any attempt or handoff.
fn job_open_deadline(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    now_ms: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT queue_deadline_ms FROM curator_jobs
          WHERE project = ?1 AND causal_identity = ?2 AND state = 'ready' AND queue_deadline_ms > ?3",
        params![project, causal_identity, now_ms],
        |row| row.get(0),
    )
    .optional()
}

/// Writes the receipt at generation 1 for a Ready job, or reports the receipt it already has. The run deadline is the creation time of the first claim ever taken on the job plus [`CURATOR_RUN_DEADLINE_MS`]; a begin at or after it is refused. The store's own incarnation is read inside the transaction; the caller supplies the Kernel incarnation it validated, which must be the one a staged subject was sealed under.
pub fn begin_curator_receipt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    kernel_incarnation_id: &str,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<CuratorBeginOutcome> {
    if !is_lower_hex(kernel_incarnation_id, 32) || claim_id.is_empty() || claim_id.len() > 200 {
        return Err(refuse(CuratorLedgerRefusal::InvalidRequest));
    }
    let incarnation = store_incarnation_in_tx(conn)?;
    let (authority_context_store, authority) = current_authority(conn, project)?
        .filter(|(context_store, _)| context_store.len() <= MAX_AUTHORITY_CONTEXT_STORE_BYTES)
        .ok_or_else(|| refuse(CuratorLedgerRefusal::AuthorityChanged))?;
    if let Some(existing) = load_receipt(conn, project, causal_identity)? {
        if existing.database_incarnation_id != incarnation
            || existing.kernel_incarnation_id != kernel_incarnation_id
            || !authority_matches(&existing, Some((authority_context_store, authority)))
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
    // A staged subject belongs to one Kernel incarnation; a receipt bound to another would let it proceed under a replaced store.
    if let ReviewTarget::StagedSubject {
        kernel_incarnation, ..
    } = &job.target
        && kernel_incarnation != kernel_incarnation_id
    {
        return Err(refuse(CuratorLedgerRefusal::BindingMismatch));
    }
    if !claim_is_live(conn, project, causal_identity, claim_id, now_ms)? {
        return Err(refuse(CuratorLedgerRefusal::ClaimInvalid));
    }
    // The budget runs from the first claim ever taken on the job, not from this call or this claim: a worker that waits before beginning spends the run, a predecessor that crashed before beginning spent part of it, and a begin with none left gets no receipt. Terminal claims outlive any run, so the earliest is always on record.
    let first_claimed_at_ms: i64 = conn.query_row(
        "SELECT MIN(created_at_ms) FROM note_eval_claims
          WHERE project = ?1 AND task_kind = ?2
            AND note_id = (SELECT job_id FROM curator_jobs WHERE project = ?1 AND causal_identity = ?3)",
        params![project, CURATOR_REVIEW_TASK.task_kind, causal_identity],
        |row| row.get(0),
    )?;
    // A begin dated before the claim that authorizes it is a clock that stepped back; nothing in the ledger yet would catch it later.
    if now_ms < first_claimed_at_ms {
        return Err(refuse(CuratorLedgerRefusal::ClockBehind));
    }
    let run_deadline_ms = first_claimed_at_ms
        .checked_add(CURATOR_RUN_DEADLINE_MS)
        .ok_or_else(|| refuse(CuratorLedgerRefusal::InvalidRequest))?;
    if now_ms >= run_deadline_ms {
        return Err(refuse(CuratorLedgerRefusal::Cutoff));
    }
    conn.execute(
        "INSERT INTO curator_receipts (
             project, causal_identity, database_incarnation_id, kernel_incarnation_id,
             authority_generation, authority_context_store, state, generation, claim_id,
             run_deadline_ms, execution_cutoff_ms, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?10, 'in_progress', 1, ?6, ?7, ?8, ?9, ?9)",
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
            authority_context_store,
        ],
    )?;
    load_receipt(conn, project, causal_identity)?
        .map(CuratorBeginOutcome::Begun)
        .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))
}

/// Adopts an in-progress receipt at `predecessor_generation` under the next generation with a new live claim. Deadlines are not in the update, so they are inherited unchanged; a takeover at or past the cutoff succeeds but can start no attempt. The receipt's bound authority row (context store and generation) must still be current: a claim acquired after the `memories` authority moved cannot adopt a receipt begun under the old one, so no completion publishes across that change.
pub fn take_over_curator_receipt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    predecessor_generation: u64,
    claim_id: &str,
    now_ms: i64,
) -> rusqlite::Result<CuratorReceipt> {
    let predecessor = generation_param(predecessor_generation).map_err(refuse)?;
    if !claim_is_live(conn, project, causal_identity, claim_id, now_ms)? {
        return Err(refuse(CuratorLedgerRefusal::ClaimInvalid));
    }
    let receipt = load_receipt(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))?;
    if !authority_matches(&receipt, current_authority(conn, project)?) {
        return Err(refuse(CuratorLedgerRefusal::AuthorityChanged));
    }
    // The claim that already owns the receipt has nothing to take over: the call replays the receipt unchanged rather than fencing its own generation and growing the ledger.
    if receipt.terminal.is_none() && receipt.claim_id == claim_id {
        return Ok(receipt);
    }
    // A takeover dated before the ledger's newest instant would move the receipt's own timestamp backwards.
    if now_ms < ledger_clock_floor(conn, project, causal_identity)? {
        return Err(refuse(CuratorLedgerRefusal::ClockBehind));
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
    if !is_lower_hex(&marker.body_digest, 64)
        || !is_lower_hex(&marker.policy_union_digest, 64)
        || marker.request_bytes == 0
        || marker.request_bytes > CURATOR_MAX_REQUEST_BYTES
        || marker.provider.is_empty()
        || marker.provider.len() > MAX_PROVIDER_BYTES
        || marker.model.is_empty()
        || marker.model.len() > MAX_MODEL_BYTES
        || marker.credential_id.is_empty()
        || marker.credential_id.len() > MAX_CREDENTIAL_ID_BYTES
    {
        return Err(refuse(CuratorLedgerRefusal::InvalidRequest));
    }
    let receipt = load_receipt(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))?;
    if receipt.terminal.is_some() || i64::try_from(receipt.generation).ok() != Some(generation) {
        return Err(refuse(CuratorLedgerRefusal::Fenced));
    }
    if receipt.claim_id != claim_id {
        return Err(refuse(CuratorLedgerRefusal::ClaimInvalid));
    }
    let claim_expires_at = live_claim_expiry(conn, project, causal_identity, claim_id, now_ms)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::ClaimInvalid))?;
    if receipt.database_incarnation_id != store_incarnation_in_tx(conn)?
        || receipt.kernel_incarnation_id != kernel_incarnation_id
    {
        return Err(refuse(CuratorLedgerRefusal::BindingMismatch));
    }
    if !authority_matches(&receipt, current_authority(conn, project)?) {
        return Err(refuse(CuratorLedgerRefusal::AuthorityChanged));
    }
    if receipt.cancelled_at_ms.is_some() {
        return Err(refuse(CuratorLedgerRefusal::Cancelled));
    }
    if now_ms >= receipt.execution_cutoff_ms {
        return Err(refuse(CuratorLedgerRefusal::Cutoff));
    }
    let job_deadline_ms = job_open_deadline(conn, project, causal_identity, now_ms)?
        .ok_or_else(|| refuse(CuratorLedgerRefusal::JobDeadline))?;
    // An attempt never outlives the cutoff, the job's queue deadline, or the claim that must record its result.
    let attempt_deadline_ms = (now_ms + CURATOR_ATTEMPT_MAX_MS)
        .min(receipt.execution_cutoff_ms)
        .min(job_deadline_ms)
        .min(claim_expires_at);
    let request_bytes = i64::try_from(marker.request_bytes)
        .map_err(|_| refuse(CuratorLedgerRefusal::InvalidRequest))?;
    // A lagging clock is refused by name: it leaves the allowance untouched, unlike exhaustion. The floor is the newest instant the ledger holds for the job.
    if now_ms < ledger_clock_floor(conn, project, causal_identity)? {
        return Err(refuse(CuratorLedgerRefusal::ClockBehind));
    }
    // The allowance and the clock floor are evaluated inside the statement: at most four markers across every generation, and no marker dated before an earlier commit or terminal.
    let attempt_index: Option<i64> = conn
        .query_row(
            "INSERT INTO curator_attempts (
                 project, causal_identity, generation, attempt_index, body_digest, request_bytes,
                 provider, model, credential_id, policy_union_digest, attempt_deadline_ms, committed_at_ms
             )
             SELECT ?1, ?2, ?3, COALESCE(MAX(attempt_index) + 1, 0), ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
               FROM curator_attempts WHERE project = ?1 AND causal_identity = ?2 AND generation = ?3
              HAVING (SELECT COUNT(*) FROM curator_attempts
                       WHERE project = ?1 AND causal_identity = ?2) < ?12
                 AND ?11 >= COALESCE((SELECT MAX(MAX(committed_at_ms), COALESCE(MAX(terminal_at_ms), 0))
                                        FROM curator_attempts
                                       WHERE project = ?1 AND causal_identity = ?2), 0)
             RETURNING attempt_index",
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
            |row| row.get(0),
        )
        .optional()?;
    let attempt_index =
        attempt_index.ok_or_else(|| refuse(CuratorLedgerRefusal::AttemptsExhausted))?;
    Ok(CuratorAttempt {
        generation: receipt.generation,
        attempt_index: u32::try_from(attempt_index).unwrap_or(u32::MAX),
        marker: marker.clone(),
        attempt_deadline_ms,
        committed_at_ms: now_ms,
        terminal: None,
    })
}

/// Records a dispatched attempt's terminal under the claim that owns the receipt's current generation, so a predecessor cannot stamp an outcome on a successor's marker. `NotDispatched` is proof that no bytes were sent; only [`MemoryStore::dispatch_curator_attempt`] knows that, so it is refused here.
#[allow(clippy::too_many_arguments)]
pub fn finish_curator_attempt_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    generation: u64,
    claim_id: &str,
    attempt_index: u32,
    terminal: CuratorAttemptTerminal,
    now_ms: i64,
) -> rusqlite::Result<()> {
    if terminal == CuratorAttemptTerminal::NotDispatched {
        return Err(refuse(CuratorLedgerRefusal::InvalidRequest));
    }
    // A terminal dated before the marker it closes is a clock that stepped back, refused by name like a lagging commit. A result that arrives at or after the attempt's own deadline is over budget: it cannot close the marker as complete, only as failed, cancelled, or unknown.
    let window: Option<(i64, i64)> = conn
        .query_row(
            "SELECT committed_at_ms, attempt_deadline_ms FROM curator_attempts
              WHERE project = ?1 AND causal_identity = ?2 AND generation = ?3 AND attempt_index = ?4",
            params![
                project,
                causal_identity,
                generation_param(generation).map_err(refuse)?,
                i64::from(attempt_index)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((_, deadline)) = window {
        // The floor is the newest instant the ledger holds for the job, not only this marker's commit: a terminal dated before a later marker, terminal, takeover, or claim is a clock behind the ledger.
        if now_ms < ledger_clock_floor(conn, project, causal_identity)? {
            return Err(refuse(CuratorLedgerRefusal::ClockBehind));
        }
        if terminal == CuratorAttemptTerminal::Complete && now_ms >= deadline {
            return Err(refuse(CuratorLedgerRefusal::Cutoff));
        }
    }
    record_attempt_terminal_in_tx(
        conn,
        project,
        causal_identity,
        generation,
        claim_id,
        attempt_index,
        terminal,
        now_ms,
    )
}

#[allow(clippy::too_many_arguments)]
fn record_attempt_terminal_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    generation: u64,
    claim_id: &str,
    attempt_index: u32,
    terminal: CuratorAttemptTerminal,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let changed = conn.execute(
        "UPDATE curator_attempts SET terminal_kind = ?6, terminal_at_ms = ?7
          WHERE project = ?1 AND causal_identity = ?2 AND generation = ?3 AND attempt_index = ?5
            AND terminal_kind IS NULL
            AND EXISTS(SELECT 1 FROM curator_receipts r
                        WHERE r.project = ?1 AND r.causal_identity = ?2
                          AND r.generation = ?3 AND r.claim_id = ?4)",
        params![
            project,
            causal_identity,
            generation_param(generation).map_err(refuse)?,
            claim_id,
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
        let terminal_kind = terminal_kind
            .map(|kind| {
                CuratorAttemptTerminal::parse(&kind).ok_or_else(|| {
                    rusqlite::Error::InvalidColumnType(10, kind, rusqlite::types::Type::Text)
                })
            })
            .transpose()?;
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

impl MemoryStore {
    fn ledger_transaction<T>(
        &self,
        project: &str,
        operation: &str,
        causal_identity: &str,
        body: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<WriteDisposition<T>>,
    ) -> Result<T, CuratorLedgerError> {
        let write = curator_write(project, &["curator-ledger", operation, causal_identity])?;
        let refusal = std::cell::Cell::new(None);
        let result = write.execute(&self.inner, |coordinated| {
            body(coordinated.tx()).inspect_err(|error| refusal.set(refusal_of(error)))
        });
        match (result, refusal.get()) {
            (Err(_), Some(refusal)) => Err(CuratorLedgerError::Refused(refusal)),
            (Err(error), None) => Err(CuratorLedgerError::Store(error)),
            (Ok(value), _) => Ok(value),
        }
    }

    /// Leases one Ready job for a worker slot through the shared task-lease ledger; the claim's task id is the job's `job_id`. A clock behind the job's own timestamps leases nothing, so no claim or receipt is ever dated before the job it belongs to.
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
        check_project(project)?;
        if !is_lower_hex(causal_identity, 64) {
            return Err(MemoryStoreError::Serde(
                "a causal identity is a 64-character lowercase hex digest".to_string(),
            ));
        }
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
                        "SELECT job_id FROM curator_jobs
                          WHERE project = ?1 AND causal_identity = ?2 AND state = 'ready'
                            AND queue_deadline_ms > ?3 AND updated_at_ms <= ?3
                            AND NOT EXISTS(SELECT 1 FROM note_eval_claims c
                                            WHERE c.project = ?1 AND c.task_kind = ?4
                                              AND c.note_id = curator_jobs.job_id
                                              AND c.terminal_kind IS NULL)",
                        params![project, identity, now_ms, CURATOR_REVIEW_TASK.task_kind],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(match row {
                    Some(job_id) => LeaseSelected::Claim {
                        note_id: job_id,
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
            |tx, job_id| {
                tx.query_row(
                    "SELECT causal_identity FROM curator_jobs WHERE job_id = ?1",
                    [job_id],
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
        check_project(project)?;
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
            // A takeover by the claim that already owns the receipt is a replay: it writes nothing and records no audit.
            let already_owned = load_receipt(conn, project, causal_identity)?
                .is_some_and(|receipt| receipt.terminal.is_none() && receipt.claim_id == claim_id);
            let receipt = take_over_curator_receipt_in_tx(
                conn,
                project,
                causal_identity,
                predecessor_generation,
                claim_id,
                now_ms,
            )?;
            Ok(if already_owned {
                WriteDisposition::Replay(receipt)
            } else {
                WriteDisposition::Applied(receipt)
            })
        })
    }

    /// Records the cancellation request; no later attempt commits and the next handoff recheck withholds. Refused at or after the run deadline, when the receipt is expired whether or not the sweep has said so yet.
    pub fn cancel_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
        now_ms: i64,
    ) -> Result<(), CuratorLedgerError> {
        self.ledger_transaction(project, "cancel", causal_identity, |conn| {
            let receipt = load_receipt(conn, project, causal_identity)?
                .ok_or_else(|| refuse(CuratorLedgerRefusal::Missing))?;
            if receipt.terminal.is_some() {
                return Err(refuse(CuratorLedgerRefusal::Fenced));
            }
            if receipt.cancelled_at_ms.is_some() {
                return Ok(WriteDisposition::Replay(()));
            }
            // Past the run deadline the run is already over; a late cancellation must not turn its `expired` into `cancelled`. A cancellation dated before the ledger's newest instant is a clock that stepped back.
            if now_ms >= receipt.run_deadline_ms {
                return Err(refuse(CuratorLedgerRefusal::Cutoff));
            }
            if now_ms < ledger_clock_floor(conn, project, causal_identity)? {
                return Err(refuse(CuratorLedgerRefusal::ClockBehind));
            }
            conn.execute(
                "UPDATE curator_receipts SET cancelled_at_ms = ?3, updated_at_ms = ?3
                  WHERE project = ?1 AND causal_identity = ?2 AND state = 'in_progress'",
                params![project, causal_identity, now_ms],
            )?;
            Ok(WriteDisposition::Applied(()))
        })
    }

    /// Commits the marker and, in the same connection ownership, hands the prepared request off once. `now` is read again after the commit: a claim, attempt deadline, cutoff, job deadline, or cancellation that lapsed in between leaves the charged marker, sends nothing, and finishes the marker `not_dispatched`. An `Err` means the marker did not commit; a failure after the commit that keeps the recheck or the handoff from running is a charged unsent attempt. The caller has already taken the Kernel guard and must not call either store from `handoff`.
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
        // The marker's caller text is prepared and audited like every other Curator identity; the handoff commits with the marker and its scan evidence.
        let mut write = curator_write(project, &["curator-ledger", "dispatch", causal_identity])?;
        write.identity("provider", &marker.provider)?;
        write.identity("model", &marker.model)?;
        write.identity("credential_id", &marker.credential_id)?;
        let refusal = std::cell::Cell::new(None);
        let result = write.execute_then_handoff(
            &self.inner,
            |coordinated| {
                commit_curator_attempt_in_tx(
                    coordinated.tx(),
                    project,
                    causal_identity,
                    generation,
                    claim_id,
                    kernel_incarnation_id,
                    marker,
                    now(),
                )
                .map(WriteDisposition::Applied)
                .inspect_err(|error| refusal.set(refusal_of(error)))
            },
            |conn, attempt| {
                // The recheck never fails the operation: an unreadable ledger after commit is a withheld handoff, not a lost marker.
                let live = now();
                let recheck = (|| -> rusqlite::Result<Option<CuratorLedgerRefusal>> {
                    let receipt = load_receipt(conn, project, causal_identity)?;
                    Ok(match receipt {
                        None => Some(CuratorLedgerRefusal::Missing),
                        // A clock that reads earlier than the marker it just committed cannot judge any deadline below; the charged attempt is withheld.
                        Some(_) if live < attempt.committed_at_ms => {
                            Some(CuratorLedgerRefusal::ClockBehind)
                        }
                        // The binding the commit checked is checked again: a receipt taken over, completed, or rebound since then owns this marker no longer.
                        Some(receipt)
                            if receipt.terminal.is_some()
                                || receipt.generation != generation
                                || receipt.claim_id != claim_id =>
                        {
                            Some(CuratorLedgerRefusal::Fenced)
                        }
                        Some(receipt) if receipt.cancelled_at_ms.is_some() => {
                            Some(CuratorLedgerRefusal::Cancelled)
                        }
                        Some(_)
                            if !claim_is_live(conn, project, causal_identity, claim_id, live)? =>
                        {
                            Some(CuratorLedgerRefusal::ClaimInvalid)
                        }
                        Some(receipt)
                            if live >= attempt.attempt_deadline_ms
                                || live >= receipt.execution_cutoff_ms =>
                        {
                            Some(CuratorLedgerRefusal::Cutoff)
                        }
                        Some(_)
                            if job_open_deadline(conn, project, causal_identity, live)?
                                .is_none() =>
                        {
                            Some(CuratorLedgerRefusal::JobDeadline)
                        }
                        Some(_) => None,
                    })
                })()
                .unwrap_or(Some(CuratorLedgerRefusal::RecheckUnavailable));
                match recheck {
                    None => Ok(handoff(prepared)),
                    Some(reason) => Err(reason),
                }
            },
        );
        // A committed marker is reported as charged whatever happened afterward; only a failure before the commit is an error, so the caller never re-charges a marker it already holds.
        let (attempt, reason) = match (result, refusal.get()) {
            (Err(_), Some(refusal)) => return Err(CuratorLedgerError::Refused(refusal)),
            (Err(error), None) => return Err(CuratorLedgerError::Store(error)),
            (
                Ok((
                    attempt,
                    HandoffOutcome::Handed {
                        handed: Ok(handoff),
                        release,
                    },
                )),
                _,
            ) => {
                return Ok(DispatchOutcome::Handed {
                    attempt_index: attempt.attempt_index,
                    attempt_deadline_ms: attempt.attempt_deadline_ms,
                    handoff,
                    release: release.map_err(MemoryStoreError::from),
                });
            }
            (
                Ok((
                    attempt,
                    HandoffOutcome::Handed {
                        handed: Err(reason),
                        ..
                    },
                )),
                _,
            ) => (attempt, reason),
            (Ok((attempt, HandoffOutcome::NotRun(_))), _) => {
                (attempt, CuratorLedgerRefusal::RecheckUnavailable)
            }
        };
        let finished = self
            .ledger_transaction(project, "finish-attempt", causal_identity, |conn| {
                // The proof is never dated before anything the ledger already holds, whatever the clock did in between; the floor is read inside this transaction so a marker another thread committed since the handoff counts too.
                let floor = ledger_clock_floor(conn, project, causal_identity)?;
                record_attempt_terminal_in_tx(
                    conn,
                    project,
                    causal_identity,
                    generation,
                    claim_id,
                    attempt.attempt_index,
                    CuratorAttemptTerminal::NotDispatched,
                    now().max(floor),
                )
                .map(WriteDisposition::Applied)
            })
            .is_ok();
        Ok(DispatchOutcome::ChargedNotDispatched {
            attempt_index: attempt.attempt_index,
            reason,
            finished,
        })
    }

    /// Records a dispatched attempt's terminal under the claim holding the current generation. `NotDispatched` is refused: the dispatch path alone knows nothing was sent.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_curator_attempt(
        &self,
        project: &str,
        causal_identity: &str,
        generation: u64,
        claim_id: &str,
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
                claim_id,
                attempt_index,
                terminal,
                now_ms,
            )
            .map(WriteDisposition::Applied)
        })
    }

    /// Completes the task lease and the receipt in one fenced transaction. The receipt must be in progress at `generation` under `claim_id` and bound to the caller's Kernel incarnation and this store's own; a `Complete` completion selects exactly one Kernel result for that generation, requires an attempt this generation closed `complete`, and the job row records the matching outcome. A predecessor generation, a stale claim, another incarnation, or a job another owner already closed or that has passed its queue deadline writes nothing. A cancelled receipt records `cancelled`, and one at or after its run deadline records `expired`, whatever the worker reports: a durable cancellation is never overridden and a renewed claim never extends the fixed run budget. The caller reads the receipt back to learn which terminal was recorded.
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
        kernel_incarnation_id: &str,
        completion: &ReceiptCompletion,
        now_ms: i64,
    ) -> Result<LeaseCompleteOutcome, MemoryStoreError> {
        let terminal = completion.terminal();
        let selection = completion.selection();
        // `Cancelled` and `Expired` are derived from the receipt's own state below; a worker cannot supply them.
        if matches!(
            terminal,
            CuratorReceiptTerminal::Cancelled | CuratorReceiptTerminal::Expired
        ) || selection.is_some_and(|selection| {
            !is_lower_hex(&selection.payload_digest, 64)
                || selection.candidate_id.is_empty()
                || selection.candidate_id.len() > 256
        }) {
            return Ok(LeaseCompleteOutcome::Conflict { kind: "invalid" });
        }
        let generation = generation_param(generation).map_err(|_| {
            MemoryStoreError::Serde("generation exceeds the storable range".to_string())
        })?;
        // A caller holding another generation or another claim is fenced before the lease is touched, whether or not the receipt is already terminal, so a stale worker's completion ends nothing and a claim on another job cannot be spent here.
        let Some(receipt) = self
            .lookup_curator_receipt(project, causal_identity)?
            .filter(|receipt| {
                i64::try_from(receipt.generation).ok() == Some(generation)
                    && receipt.claim_id == claim_id
            })
        else {
            return Ok(LeaseCompleteOutcome::Conflict { kind: "fenced" });
        };
        // A claim already terminal has nothing left to decide: the lease ledger replays its stored response or reports the conflict, and the clock and evidence checks for a new completion do not apply.
        let (claim_terminal, floor): (bool, i64) = self.inner.with_conn(|conn| {
            let terminal: Option<bool> = conn
                .query_row(
                    "SELECT terminal_kind IS NOT NULL FROM note_eval_claims
                      WHERE project = ?1 AND task_kind = ?2 AND claim_id = ?3",
                    params![project, CURATOR_REVIEW_TASK.task_kind, claim_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok((
                terminal.unwrap_or(false),
                ledger_clock_floor(conn, project, causal_identity)?,
            ))
        })?;
        if !claim_terminal {
            // The receipt's Kernel binding is checked here as well, so a mismatch is a refusal rather than a stale write that ends the claim.
            if receipt.kernel_incarnation_id != kernel_incarnation_id {
                return Ok(LeaseCompleteOutcome::Conflict {
                    kind: "binding_mismatch",
                });
            }
            // A completion dated before any instant the ledger already holds for the job is a clock that stepped back, whatever terminal it reports; the claim survives so the worker can retry once it catches up.
            if now_ms < floor {
                return Ok(LeaseCompleteOutcome::Conflict {
                    kind: "clock_behind",
                });
            }
        }
        let attempts = self.list_curator_attempts(project, causal_identity)?;
        // A published result must come from an attempt this generation closed `complete` inside its budget; the store has no other evidence the selection exists. A complete terminal never reverts, so this read outside the lease is authoritative, and the claim survives the refusal. A receipt already cancelled or past its run deadline records that instead, so it needs no evidence here; the transaction below reads the row again before it decides.
        let publishing = terminal == CuratorReceiptTerminal::Complete
            && receipt.cancelled_at_ms.is_none()
            && now_ms < receipt.run_deadline_ms;
        if !claim_terminal
            && publishing
            && !attempts.iter().any(|attempt| {
                i64::try_from(attempt.generation).ok() == Some(generation)
                    && matches!(
                        attempt.terminal,
                        Some((CuratorAttemptTerminal::Complete, _))
                    )
            })
        {
            return Ok(LeaseCompleteOutcome::Conflict { kind: "invalid" });
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
                // Read inside the transaction: a cancellation that landed since the fence check still wins, and the run deadline is authoritative from any read.
                let Some(receipt) = load_receipt(tx, project, causal_identity)? else {
                    return Ok(LeaseCompletion::Stale);
                };
                let (terminal, selection, abstained_reason) = if receipt.cancelled_at_ms.is_some() {
                    (CuratorReceiptTerminal::Cancelled, None, None)
                } else if now_ms >= receipt.run_deadline_ms {
                    (CuratorReceiptTerminal::Expired, None, None)
                } else {
                    (terminal, selection, completion.abstained_reason())
                };
                // The clock and evidence checks made before the lease are repeated here, serialized with the write: a dispatch that landed in between leaves a newer event, and a completion dated before it, or a publication no longer backed, is stale rather than written.
                let backed: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM curator_attempts
                                    WHERE project = ?1 AND causal_identity = ?2
                                      AND generation = ?3 AND terminal_kind = 'complete')",
                    params![project, causal_identity, generation],
                    |row| row.get(0),
                )?;
                if now_ms < ledger_clock_floor(tx, project, causal_identity)?
                    || (terminal == CuratorReceiptTerminal::Complete && !backed)
                {
                    return Ok(LeaseCompletion::Stale);
                }
                // The candidate id is caller text the receipt keeps for the store incarnation; it is scanned like every other Curator identity, and the table trigger refuses it again. The receipt owns the scan beside the claim, so the evidence outlives the claim's retention window.
                if selection.is_some() {
                    coordinated.domain_owner(
                        "project",
                        project,
                        active_scan_owner_key(&["curator-ledger", "complete", causal_identity]),
                    );
                }
                let candidate_id = selection
                    .map(|selection| {
                        coordinated
                            .prepared
                            .borrow_mut()
                            .transaction_identity("selected_candidate_id", &selection.candidate_id)
                            .map_err(redaction_error)
                    })
                    .transpose()?;
                let changed = tx.execute(
                    "UPDATE curator_receipts
                        SET state = 'complete', terminal_kind = ?5, selected_generation = ?6,
                            selected_candidate_id = ?7, selected_payload_digest = ?8, updated_at_ms = ?9,
                            abstained_reason = ?11
                      WHERE project = ?1 AND causal_identity = ?2 AND state = 'in_progress'
                        AND generation = ?3 AND claim_id = ?4
                        AND kernel_incarnation_id = ?10
                        AND database_incarnation_id = (SELECT database_incarnation_id
                                                       FROM curator_store_identity WHERE id = 0)
                        AND EXISTS(SELECT 1 FROM curator_jobs j
                                    WHERE j.project = ?1 AND j.causal_identity = ?2
                                      AND j.state <> 'terminal' AND j.queue_deadline_ms > ?9)",
                    params![
                        project,
                        causal_identity,
                        generation,
                        claim_id,
                        terminal.as_str(),
                        selection.map(|_| generation),
                        candidate_id,
                        selection.map(|selection| selection.payload_digest.as_str()),
                        now_ms,
                        kernel_incarnation_id,
                        abstained_reason.map(AbstainReason::as_str),
                    ],
                )?;
                // A job already closed by another owner, or past its queue deadline, leaves this completion stale with the receipt untouched; the update above requires the job to be open, so the job finish below cannot refuse it as terminal or expired.
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

    /// Completed receipts of `project` in causal-identity order, at most `limit`, after `after` when given. A page is one bounded read; the caller copies it and releases this store before entering the Kernel.
    pub fn list_completed_curator_receipts(
        &self,
        project: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CuratorReceipt>, MemoryStoreError> {
        check_project(project)?;
        let limit = limit.clamp(1, MAX_RECEIPT_PAGE) as i64;
        self.inner
            .with_conn(|conn| {
                let mut statement = conn.prepare_cached(&format!(
                    "SELECT {RECEIPT_COLUMNS} FROM curator_receipts
                      WHERE project = ?1 AND state = 'complete' AND causal_identity > ?2
                      ORDER BY causal_identity LIMIT ?3"
                ))?;
                let rows = statement.query_map(
                    params![project, after.unwrap_or(""), limit],
                    receipt_from_row,
                )?;
                rows.collect()
            })
            .map_err(Into::into)
    }

    pub fn lookup_curator_receipt(
        &self,
        project: &str,
        causal_identity: &str,
    ) -> Result<Option<CuratorReceipt>, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| load_receipt(conn, project, causal_identity))
            .map_err(Into::into)
    }

    pub fn list_curator_attempts(
        &self,
        project: &str,
        causal_identity: &str,
    ) -> Result<Vec<CuratorAttempt>, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| list_curator_attempts_in_tx(conn, project, causal_identity))
            .map_err(Into::into)
    }
}
