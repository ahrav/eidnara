//! Durable, kind-parameterized task leases: acquire, renew, complete, and
//! abandon with lease expiry and registration-generation fencing.
//!
//! One worker slot holds at most one active claim per task kind and project;
//! one task is claimed by at most one active claim. Every mutating call is
//! keyed by a client-chosen identity (acquisition id, claim id, completion
//! id) and replays its recorded decision when that identity is seen again,
//! so a client that lost a response can retry without a second effect.
//!
//! All kinds share the `note_eval_claims` and `note_eval_acquisitions`
//! tables and are separated by the `task_kind` column; column names are
//! inherited from the first instantiation (note evaluation), so a kind's task
//! identity is stored in `note_id` and its worker identity in
//! `evaluator_instance` / `evaluator_slot`. What a task is, which one is
//! selected, and what its completion writes belong to the kind and enter this
//! module only through the closures `acquire_task_lease` and
//! `complete_task_lease` take.
//!
//! Every kind is fenced on the project's `notes` authority row: its MODULE
//! generation is recorded on each claim and revalidated on renew and
//! complete, and an authority change fences every kind's active claims at
//! once. Kinds differ in their task identity, phases, and retention only.

use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use storage::GuardedConn;

use crate::{
    ActiveWriteTransaction, DurableWriteFamily, MemoryStore, MemoryStoreError, PreparedWrite,
    WriteDisposition, active_scan_owner_key, retire_active_scan_domain_owner,
};

/// The write family every task-lease ledger writes under.
const LEDGER_FAMILY: DurableWriteFamily = DurableWriteFamily::NoteEvaluationLedgers;

const ID_MAX_LEN: usize = 200;
const RESPONSE_MAX_LEN: usize = 2048;

const CLAIM_COLUMNS: &str = "claim_id, note_id, phase, acquisition_id, \
    evaluator_instance, evaluator_slot, registration_generation, source_revision, \
    state_version, policy_version, protocol_epoch, authority_generation, expires_at, \
    completion_id, terminal_kind, terminal_response";

/// The static description of one task kind's lease ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLeaseKind {
    /// The `task_kind` discriminator stored on every row.
    pub task_kind: &'static str,
    /// Prefix of every claim id this kind issues.
    pub claim_id_prefix: &'static str,
    /// Phases a selector may claim.
    pub phases: &'static [&'static str],
    pub lease_ms: i64,
    /// Replay retention for a `no_work` acquisition decision.
    pub no_work_retention_ms: i64,
    /// Replay retention for a terminal claim result.
    pub terminal_retention_ms: i64,
    /// How long a terminal claim keeps its response before redaction.
    pub response_redact_ms: i64,
    /// Cap per (project, kind) on in-flight claims and on live `no_work`
    /// decisions.
    pub ledger_cap: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseClaim {
    pub claim_id: String,
    pub note_id: i64,
    pub phase: String,
    pub acquisition_id: String,
    pub evaluator_instance: String,
    pub evaluator_slot: i64,
    pub registration_generation: i64,
    pub source_revision: i64,
    pub state_version: i64,
    pub policy_version: i64,
    pub protocol_epoch: i64,
    pub authority_generation: i64,
    pub expires_at: i64,
}

/// What a kind's selector decided inside the acquisition transaction. The
/// three versions bind the claim to the task snapshot; completion is fenced on
/// them.
pub enum LeaseSelected<T> {
    Claim {
        note_id: i64,
        phase: String,
        task: T,
        source_revision: i64,
        state_version: i64,
        policy_version: i64,
    },
    NoWork {
        cycle_exhausted: bool,
    },
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAcquireOutcome<T> {
    Claim {
        claim: LeaseClaim,
        task: T,
        replayed: bool,
    },
    NoWork {
        replayed: bool,
        /// The selection cursor, not the queue, ended this pass. Durable in
        /// the acquisition ledger so response-loss replays re-announce it.
        cycle_exhausted: bool,
    },
    /// The acquisition identity replays an expired decision.
    Expired,
    /// The acquisition identity replays a terminal claim result.
    Terminal {
        kind: String,
        response: Option<String>,
    },
    Busy,
    AuthorityChanged,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseRenewOutcome {
    Renewed {
        expires_at: i64,
    },
    Expired,
    AuthorityChanged,
    UnknownClaim,
    Invalid,
    TerminalReplay {
        kind: String,
        response: Option<String>,
    },
}

/// What a kind's completion body did once every lease fence held.
pub enum LeaseCompletion {
    Applied {
        response_json: String,
    },
    /// The task moved underneath the claim; nothing was written.
    Stale,
    /// The completion was rejected; `response` is the replayable terminal body.
    Invalid {
        response: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseCompleteOutcome {
    Applied { response_json: String },
    Replayed { response_json: String },
    Conflict { kind: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAbandonOutcome {
    Abandoned,
    Replayed { kind: String },
    UnknownClaim,
    Invalid,
}

struct ClaimRow {
    claim: LeaseClaim,
    completion_id: Option<String>,
    terminal_kind: Option<String>,
    terminal_response: Option<String>,
}

fn claim_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClaimRow> {
    Ok(ClaimRow {
        claim: LeaseClaim {
            claim_id: row.get(0)?,
            note_id: row.get(1)?,
            phase: row.get(2)?,
            acquisition_id: row.get(3)?,
            evaluator_instance: row.get(4)?,
            evaluator_slot: row.get(5)?,
            registration_generation: row.get(6)?,
            source_revision: row.get(7)?,
            state_version: row.get(8)?,
            policy_version: row.get(9)?,
            protocol_epoch: row.get(10)?,
            authority_generation: row.get(11)?,
            expires_at: row.get(12)?,
        },
        completion_id: row.get(13)?,
        terminal_kind: row.get(14)?,
        terminal_response: row.get(15)?,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= ID_MAX_LEN
}

pub(crate) fn bound_response(response: &str) -> String {
    if response.len() <= RESPONSE_MAX_LEN {
        return response.to_string();
    }
    let mut end = RESPONSE_MAX_LEN;
    while !response.is_char_boundary(end) {
        end -= 1;
    }
    response[..end].to_string()
}

pub(crate) fn kind_response(kind: &str) -> String {
    serde_json::json!({ "result": kind }).to_string()
}

pub(crate) fn redaction_error(error: MemoryStoreError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

/// Resolve the notes-authority MODULE row every kind's protocol is fenced on.
/// A MODULE row wins over stale twins under other context store UUIDs,
/// matching `module_authority_for_project`. Returns `(generation, epoch)`.
fn module_authority_tx(
    tx: &GuardedConn<'_>,
    project: &str,
) -> rusqlite::Result<Option<(i64, i64)>> {
    tx.query_row(
        "SELECT generation, note_eval_protocol_epoch
           FROM authority
          WHERE project = ?1 AND domain = 'notes' AND state = 'MODULE'
          ORDER BY context_store_uuid
          LIMIT 1",
        params![project],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

fn load_claim_tx(
    tx: &GuardedConn<'_>,
    kind: &TaskLeaseKind,
    project: &str,
    claim_id: &str,
) -> rusqlite::Result<Option<ClaimRow>> {
    tx.query_row(
        &format!(
            "SELECT {CLAIM_COLUMNS} FROM note_eval_claims
              WHERE project = ?1 AND task_kind = ?2 AND claim_id = ?3"
        ),
        params![project, kind.task_kind, claim_id],
        claim_from_row,
    )
    .optional()
}

#[allow(clippy::too_many_arguments)]
fn mark_terminal_tx(
    tx: &GuardedConn<'_>,
    kind: &TaskLeaseKind,
    project: &str,
    claim_id: &str,
    terminal_kind: &str,
    completion_id: Option<&str>,
    response: &str,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE note_eval_claims
            SET terminal_kind = ?1, completion_id = COALESCE(?2, completion_id),
                terminal_response = ?3, terminal_at_ms = ?4
          WHERE project = ?5 AND task_kind = ?6 AND claim_id = ?7 AND terminal_kind IS NULL",
        params![
            terminal_kind,
            completion_id,
            response,
            now_ms,
            project,
            kind.task_kind,
            claim_id
        ],
    )
}

/// Terminally fence one task's active claim so in-flight work loses its
/// completion instead of surfacing a stale result.
pub(crate) fn fence_task_claims_tx(
    tx: &GuardedConn<'_>,
    kind: &TaskLeaseKind,
    project: &str,
    note_id: i64,
    terminal_kind: &str,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE note_eval_claims
            SET terminal_kind = ?1, terminal_response = ?2, terminal_at_ms = ?3
          WHERE project = ?4 AND task_kind = ?5 AND note_id = ?6 AND terminal_kind IS NULL",
        params![
            terminal_kind,
            kind_response(terminal_kind),
            now_ms,
            project,
            kind.task_kind,
            note_id
        ],
    )
}

/// Terminally fence every kind's active claims in a project; the authority
/// every kind is fenced on changed underneath them.
pub(crate) fn fence_project_claims_tx(
    tx: &GuardedConn<'_>,
    project: &str,
    terminal_kind: &str,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE note_eval_claims
            SET terminal_kind = ?1, terminal_response = ?2, terminal_at_ms = ?3
          WHERE project = ?4 AND terminal_kind IS NULL",
        params![terminal_kind, kind_response(terminal_kind), now_ms, project],
    )
}

/// Ledger garbage collection: expire overdue active claims, tombstone expired
/// `no_work` decisions (`decision = ''` keeps replay identity), then null the
/// response of terminal claims once their completion-replay window has passed
/// (`response_redact_ms`, well before row deletion at `terminal_retention_ms`).
/// Tombstoned rows replay as `Expired`; they no longer count against either
/// cap.
fn collect_ledgers_tx(
    tx: &GuardedConn<'_>,
    kind: &TaskLeaseKind,
    project: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE note_eval_claims
            SET terminal_kind = 'expired', terminal_response = ?1, terminal_at_ms = ?2
          WHERE project = ?3 AND task_kind = ?4 AND terminal_kind IS NULL AND expires_at <= ?2",
        params![kind_response("expired"), now_ms, project, kind.task_kind],
    )?;
    tx.execute(
        "UPDATE note_eval_acquisitions SET decision = ''
          WHERE project = ?1 AND task_kind = ?2 AND decision <> '' AND expires_at <= ?3",
        params![project, kind.task_kind, now_ms],
    )?;
    tx.execute(
        "UPDATE note_eval_claims SET terminal_response = NULL
          WHERE project = ?1 AND task_kind = ?2 AND terminal_kind IS NOT NULL
            AND terminal_response IS NOT NULL
            AND terminal_at_ms IS NOT NULL AND terminal_at_ms <= ?3",
        params![project, kind.task_kind, now_ms - kind.response_redact_ms],
    )?;
    // Reclaim rows, not just columns. Blanking `decision`/`terminal_response`
    // stops a row counting toward the cap but leaves it on disk forever, so both
    // ledgers would grow without bound - one acquisition row per poll, including
    // every idle no-work poll - and the per-poll GC and cap counts would degrade
    // into ever-longer scans.
    let expired_acquisitions = {
        let mut statement = tx.prepare(
            "SELECT acquisition_id FROM note_eval_acquisitions
              WHERE project = ?1 AND task_kind = ?2 AND decision = '' AND expires_at <= ?3",
        )?;

        statement
            .query_map(
                params![project, kind.task_kind, now_ms - kind.no_work_retention_ms],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    for acquisition_id in expired_acquisitions {
        retire_active_scan_domain_owner(
            tx,
            "project",
            project,
            LEDGER_FAMILY.owner_kind(),
            &acquisition_owner_key(kind, &acquisition_id),
        )?;
    }
    tx.execute(
        "DELETE FROM note_eval_acquisitions
          WHERE project = ?1 AND task_kind = ?2 AND decision = '' AND expires_at <= ?3",
        params![project, kind.task_kind, now_ms - kind.no_work_retention_ms],
    )?;
    let expired_claims = {
        let mut statement = tx.prepare(
            "SELECT claim_id FROM note_eval_claims
              WHERE project = ?1 AND task_kind = ?2 AND terminal_kind IS NOT NULL
                AND terminal_at_ms IS NOT NULL AND terminal_at_ms <= ?3",
        )?;

        statement
            .query_map(
                params![project, kind.task_kind, now_ms - kind.terminal_retention_ms],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    for claim_id in expired_claims {
        retire_active_scan_domain_owner(
            tx,
            "project",
            project,
            LEDGER_FAMILY.owner_kind(),
            &claim_owner_key(kind, &claim_id),
        )?;
    }
    tx.execute(
        "DELETE FROM note_eval_claims
          WHERE project = ?1 AND task_kind = ?2 AND terminal_kind IS NOT NULL
            AND terminal_at_ms IS NOT NULL AND terminal_at_ms <= ?3",
        params![project, kind.task_kind, now_ms - kind.terminal_retention_ms],
    )?;
    Ok(())
}

/// Scan-audit owner keys carry the kind: acquisition ids are client-chosen and
/// claim ids are unique only within a kind, so either may repeat across kinds.
fn claim_owner_key(kind: &TaskLeaseKind, claim_id: &str) -> String {
    active_scan_owner_key(&["claim", kind.task_kind, claim_id])
}

fn acquisition_owner_key(kind: &TaskLeaseKind, acquisition_id: &str) -> String {
    active_scan_owner_key(&["acquisition", kind.task_kind, acquisition_id])
}

/// Rebind an active claim to the caller's registration and replay it with the
/// current task snapshot. The lease is refreshed alongside the rebind: a
/// re-registered worker schedules its first renewal a full heartbeat interval
/// out, so replaying a claim with its original near-expiry lease would let the
/// lease lapse mid-execution and force the billable phase to run again.
#[allow(clippy::too_many_arguments)]
fn rebind_claim_tx<T>(
    tx: &GuardedConn<'_>,
    kind: &TaskLeaseKind,
    project: &str,
    mut claim: LeaseClaim,
    acquisition_id: &str,
    registration_generation: i64,
    now_ms: i64,
    load: impl FnOnce(&GuardedConn<'_>, i64) -> rusqlite::Result<Option<T>>,
) -> rusqlite::Result<LeaseAcquireOutcome<T>> {
    let expires_at = now_ms + kind.lease_ms;
    // The slot-recovery path reaches this rebind with a NEW acquisition id.
    // Persisting it keeps the replay guarantee: a retry of that id hits the
    // acquisition-keyed lookup and replays this rebind instead of re-entering
    // slot recovery and extending the lease again.
    tx.execute(
        "UPDATE note_eval_claims
            SET registration_generation = ?1, expires_at = ?2, acquisition_id = ?3
          WHERE project = ?4 AND task_kind = ?5 AND claim_id = ?6 AND terminal_kind IS NULL",
        params![
            registration_generation,
            expires_at,
            acquisition_id,
            project,
            kind.task_kind,
            claim.claim_id
        ],
    )?;
    claim.registration_generation = registration_generation;
    claim.expires_at = expires_at;
    claim.acquisition_id = acquisition_id.to_string();
    let Some(task) = load(tx, claim.note_id)? else {
        return Ok(LeaseAcquireOutcome::Invalid);
    };
    Ok(LeaseAcquireOutcome::Claim {
        claim,
        task,
        replayed: true,
    })
}

impl MemoryStore {
    /// Acquire one durable claim or a replayable `no_work` decision. `select`
    /// runs inside the acquisition transaction and names the task to claim
    /// (with its snapshot and fence versions) or a classified `no_work`;
    /// `load` reloads a task's snapshot when an existing claim is replayed.
    /// Neither may write: outcomes they lead to other than a fresh claim or
    /// decision commit as replays, which skip the scan audit. The decision — including a `no_work`'s `cycle_exhausted` cause —
    /// commits atomically under (project, task_kind, acquisition_id)
    /// uniqueness so the same acquisition ID always returns the same decision
    /// after response loss.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn acquire_task_lease<T>(
        &self,
        kind: &TaskLeaseKind,
        project: &str,
        acquisition_id: &str,
        evaluator_instance: &str,
        evaluator_slot: i64,
        registration_generation: i64,
        now_ms: i64,
        select: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<LeaseSelected<T>>,
        load: impl Fn(&GuardedConn<'_>, i64) -> rusqlite::Result<Option<T>>,
    ) -> Result<LeaseAcquireOutcome<T>, MemoryStoreError> {
        if project.is_empty()
            || !valid_id(acquisition_id)
            || !valid_id(evaluator_instance)
            || evaluator_slot < 0
        {
            return Ok(LeaseAcquireOutcome::Invalid);
        }
        let mut write = PreparedWrite::new(LEDGER_FAMILY);
        write.existing_identity("project", project)?;
        write.identity("acquisition_id", acquisition_id)?;
        write.identity("evaluator_instance", evaluator_instance)?;
        self.with_prepared_note_conn_fenced(project, write, |coordinated| {
            let tx = coordinated.tx;
            collect_ledgers_tx(tx, kind, project, now_ms)?;
            let replayed_claim = tx
                .query_row(
                    &format!(
                        "SELECT {CLAIM_COLUMNS} FROM note_eval_claims
                          WHERE project = ?1 AND task_kind = ?2 AND acquisition_id = ?3"
                    ),
                    params![project, kind.task_kind, acquisition_id],
                    claim_from_row,
                )
                .optional()?;
            if let Some(row) = replayed_claim {
                if let Some(terminal_kind) = row.terminal_kind {
                    return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Terminal {
                        kind: terminal_kind,
                        response: row.terminal_response,
                    }));
                }
                if row.claim.evaluator_instance != evaluator_instance
                    || row.claim.evaluator_slot != evaluator_slot
                    || row.claim.registration_generation > registration_generation
                {
                    return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Invalid));
                }
                return rebind_claim_tx(
                    tx,
                    kind,
                    project,
                    row.claim,
                    acquisition_id,
                    registration_generation,
                    now_ms,
                    &load,
                )
                .map(WriteDisposition::Replay);
            }
            let replayed_decision = tx
                .query_row(
                    "SELECT decision FROM note_eval_acquisitions
                      WHERE project = ?1 AND task_kind = ?2 AND acquisition_id = ?3",
                    params![project, kind.task_kind, acquisition_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(decision) = replayed_decision {
                return Ok(WriteDisposition::Replay(if decision.is_empty() {
                    LeaseAcquireOutcome::Expired
                } else {
                    // Replay the full recorded decision: a client only sees a
                    // replay after losing the original response, so the
                    // exhaustion cause must survive or the worker mistakes a
                    // reset cursor for a drained queue.
                    LeaseAcquireOutcome::NoWork {
                        replayed: true,
                        cycle_exhausted: decision == "no_work_exhausted",
                    }
                }));
            }
            let Some((authority_generation, protocol_epoch)) = module_authority_tx(tx, project)?
            else {
                return Ok(WriteDisposition::Replay(
                    LeaseAcquireOutcome::AuthorityChanged,
                ));
            };
            let slot_claim = tx
                .query_row(
                    &format!(
                        "SELECT {CLAIM_COLUMNS} FROM note_eval_claims
                          WHERE project = ?1 AND task_kind = ?2 AND evaluator_instance = ?3
                            AND evaluator_slot = ?4 AND terminal_kind IS NULL"
                    ),
                    params![project, kind.task_kind, evaluator_instance, evaluator_slot],
                    claim_from_row,
                )
                .optional()?;
            if let Some(row) = slot_claim {
                if row.claim.registration_generation > registration_generation {
                    return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Invalid));
                }
                coordinated.domain_owner(
                    "project",
                    project,
                    claim_owner_key(kind, &row.claim.claim_id),
                );
                return rebind_claim_tx(
                    tx,
                    kind,
                    project,
                    row.claim,
                    acquisition_id,
                    registration_generation,
                    now_ms,
                    &load,
                )
                .map(WriteDisposition::Applied);
            }
            let (note_id, phase, task, source_revision, state_version, policy_version) =
                match select(tx)? {
                    LeaseSelected::Claim {
                        note_id,
                        phase,
                        task,
                        source_revision,
                        state_version,
                        policy_version,
                    } => (
                        note_id,
                        phase,
                        task,
                        source_revision,
                        state_version,
                        policy_version,
                    ),
                    LeaseSelected::Invalid => {
                        return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Invalid));
                    }
                    LeaseSelected::NoWork { cycle_exhausted } => {
                        let live: i64 = tx.query_row(
                            "SELECT COUNT(*) FROM note_eval_acquisitions
                          WHERE project = ?1 AND task_kind = ?2 AND decision <> ''",
                            params![project, kind.task_kind],
                            |row| row.get(0),
                        )?;
                        if live >= kind.ledger_cap {
                            return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Busy));
                        }
                        tx.execute(
                            "INSERT INTO note_eval_acquisitions(
                             project, task_kind, acquisition_id, decision, created_at_ms,
                             expires_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![
                                project,
                                kind.task_kind,
                                acquisition_id,
                                if cycle_exhausted {
                                    "no_work_exhausted"
                                } else {
                                    "no_work"
                                },
                                now_ms,
                                now_ms + kind.no_work_retention_ms
                            ],
                        )?;
                        coordinated.domain_owner(
                            "project",
                            project,
                            acquisition_owner_key(kind, acquisition_id),
                        );
                        return Ok(WriteDisposition::Applied(LeaseAcquireOutcome::NoWork {
                            replayed: false,
                            cycle_exhausted,
                        }));
                    }
                };
            if !kind.phases.contains(&phase.as_str()) {
                return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Invalid));
            }
            // Bound IN-FLIGHT claims only. Counting terminal rows still inside the
            // replay-retention window would turn this cap into a rolling
            // throughput ceiling: once a project completed `ledger_cap`
            // evaluations within the retention window every further acquisition
            // would return Busy and all evaluation would stall until rows aged
            // out. Terminal-row volume is bounded by deletion in
            // `collect_ledgers_tx` instead.
            let live: i64 = tx.query_row(
                "SELECT COUNT(*) FROM note_eval_claims
                  WHERE project = ?1 AND task_kind = ?2 AND terminal_kind IS NULL",
                params![project, kind.task_kind],
                |row| row.get(0),
            )?;
            if live >= kind.ledger_cap {
                return Ok(WriteDisposition::Replay(LeaseAcquireOutcome::Busy));
            }
            let claim = LeaseClaim {
                // The module's wire protocol caps id fields at 128 bytes and
                // the client echoes this id on every renew/complete/abandon,
                // so it must stay bounded regardless of how long the project
                // identity or acquisition id are. The digest keeps it
                // deterministic per (project, acquisition); a replayed
                // acquisition reads the stored row.
                claim_id: {
                    let mut hasher = Sha256::new();
                    hasher.update(project.as_bytes());
                    hasher.update([0u8]);
                    hasher.update(acquisition_id.as_bytes());
                    format!("{}{:x}", kind.claim_id_prefix, hasher.finalize())
                },
                note_id,
                phase,
                acquisition_id: acquisition_id.to_string(),
                evaluator_instance: evaluator_instance.to_string(),
                evaluator_slot,
                registration_generation,
                source_revision,
                state_version,
                policy_version,
                protocol_epoch,
                authority_generation,
                expires_at: now_ms + kind.lease_ms,
            };
            for (field_id, value) in [
                ("claim_id", claim.claim_id.as_str()),
                ("phase", claim.phase.as_str()),
            ] {
                coordinated
                    .prepared
                    .borrow_mut()
                    .transaction_identity(field_id, value)
                    .map_err(redaction_error)?;
            }
            tx.execute(
                "INSERT INTO note_eval_claims(
                     claim_id, project, task_kind, note_id, phase, acquisition_id,
                     evaluator_instance, evaluator_slot, registration_generation,
                     source_revision, state_version, policy_version, protocol_epoch,
                     authority_generation, expires_at, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    claim.claim_id,
                    project,
                    kind.task_kind,
                    claim.note_id,
                    claim.phase,
                    claim.acquisition_id,
                    claim.evaluator_instance,
                    claim.evaluator_slot,
                    claim.registration_generation,
                    claim.source_revision,
                    claim.state_version,
                    claim.policy_version,
                    claim.protocol_epoch,
                    claim.authority_generation,
                    claim.expires_at,
                    now_ms,
                ],
            )?;
            coordinated.domain_owner("project", project, claim_owner_key(kind, &claim.claim_id));
            Ok(WriteDisposition::Applied(LeaseAcquireOutcome::Claim {
                claim,
                task,
                replayed: false,
            }))
        })
    }

    /// Extend an active claim's lease by one lease interval.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn renew_task_lease(
        &self,
        kind: &TaskLeaseKind,
        project: &str,
        claim_id: &str,
        evaluator_instance: &str,
        evaluator_slot: i64,
        registration_generation: i64,
        now_ms: i64,
    ) -> Result<LeaseRenewOutcome, MemoryStoreError> {
        let mut write = PreparedWrite::new(LEDGER_FAMILY);
        write.domain_owner("project", project, claim_owner_key(kind, claim_id));
        for (field_id, value) in [
            ("project", project),
            ("claim_id", claim_id),
            ("evaluator_instance", evaluator_instance),
        ] {
            write.existing_identity(field_id, value)?;
        }
        self.with_prepared_note_conn_fenced(project, write, |coordinated| {
            let tx = coordinated.tx;
            let before = tx.total_changes();
            let outcome = (|| -> rusqlite::Result<LeaseRenewOutcome> {
                let Some(row) = load_claim_tx(tx, kind, project, claim_id)? else {
                    return Ok(LeaseRenewOutcome::UnknownClaim);
                };
                if let Some(terminal_kind) = row.terminal_kind {
                    return Ok(LeaseRenewOutcome::TerminalReplay {
                        kind: terminal_kind,
                        response: row.terminal_response,
                    });
                }
                if row.claim.evaluator_instance != evaluator_instance
                    || row.claim.evaluator_slot != evaluator_slot
                    || row.claim.registration_generation != registration_generation
                {
                    return Ok(LeaseRenewOutcome::Invalid);
                }
                if row.claim.expires_at <= now_ms {
                    mark_terminal_tx(
                        tx,
                        kind,
                        project,
                        claim_id,
                        "expired",
                        None,
                        &kind_response("expired"),
                        now_ms,
                    )?;
                    return Ok(LeaseRenewOutcome::Expired);
                }
                match module_authority_tx(tx, project)? {
                    Some((generation, _)) if generation == row.claim.authority_generation => {}
                    _ => return Ok(LeaseRenewOutcome::AuthorityChanged),
                }
                let expires_at = now_ms + kind.lease_ms;
                tx.execute(
                    "UPDATE note_eval_claims
                        SET expires_at = ?1
                      WHERE project = ?2 AND task_kind = ?3 AND claim_id = ?4
                        AND terminal_kind IS NULL",
                    params![expires_at, project, kind.task_kind, claim_id],
                )?;
                Ok(LeaseRenewOutcome::Renewed { expires_at })
            })()?;
            Ok(if tx.total_changes() > before {
                WriteDisposition::Applied(outcome)
            } else {
                WriteDisposition::Replay(outcome)
            })
        })
    }

    /// Commit one claim's outcome. Every lease fence (identity, expiry, and
    /// authority generation) is revalidated first; `apply` then runs in the
    /// same transaction, performs the kind's own snapshot fences and writes,
    /// and reports what it did. The terminal result is recorded under
    /// `completion_id` so the same completion replays after response loss.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete_task_lease(
        &self,
        kind: &TaskLeaseKind,
        project: &str,
        claim_id: &str,
        completion_id: &str,
        evaluator_instance: &str,
        evaluator_slot: i64,
        now_ms: i64,
        apply: impl FnOnce(
            &mut ActiveWriteTransaction<'_>,
            &LeaseClaim,
        ) -> rusqlite::Result<LeaseCompletion>,
    ) -> Result<LeaseCompleteOutcome, MemoryStoreError> {
        if !valid_id(completion_id) {
            return Ok(LeaseCompleteOutcome::Conflict { kind: "invalid" });
        }
        let mut write = PreparedWrite::new(LEDGER_FAMILY);
        write.domain_owner("project", project, claim_owner_key(kind, claim_id));
        for (field_id, value) in [
            ("project", project),
            ("claim_id", claim_id),
            ("completion_id", completion_id),
            ("evaluator_instance", evaluator_instance),
        ] {
            write.existing_identity(field_id, value)?;
        }
        self.with_prepared_note_conn_fenced(project, write, |coordinated| {
            let tx = coordinated.tx;
            let before = tx.total_changes();
            let outcome = (|| -> rusqlite::Result<LeaseCompleteOutcome> {
                let Some(row) = load_claim_tx(tx, kind, project, claim_id)? else {
                    return Ok(LeaseCompleteOutcome::Conflict {
                        kind: "unknown_claim",
                    });
                };
                if let Some(terminal_kind) = row.terminal_kind {
                    return Ok(match row.completion_id {
                        Some(stored) if stored == completion_id => match row.terminal_response {
                            Some(response_json) => LeaseCompleteOutcome::Replayed { response_json },
                            None => LeaseCompleteOutcome::Conflict { kind: "expired" },
                        },
                        Some(_) => LeaseCompleteOutcome::Conflict {
                            kind: "completion_conflict",
                        },
                        None => LeaseCompleteOutcome::Conflict {
                            kind: match terminal_kind.as_str() {
                                "stale" => "stale",
                                "expired" => "expired",
                                "authority_changed" => "authority_changed",
                                _ => "invalid",
                            },
                        },
                    });
                }
                if row.claim.evaluator_instance != evaluator_instance
                    || row.claim.evaluator_slot != evaluator_slot
                {
                    return Ok(LeaseCompleteOutcome::Conflict { kind: "invalid" });
                }
                let terminal = |terminal_kind: &'static str,
                                completion_id: Option<&str>,
                                response: &str|
                 -> rusqlite::Result<LeaseCompleteOutcome> {
                    mark_terminal_tx(
                        tx,
                        kind,
                        project,
                        claim_id,
                        terminal_kind,
                        completion_id,
                        response,
                        now_ms,
                    )?;
                    Ok(LeaseCompleteOutcome::Conflict {
                        kind: terminal_kind,
                    })
                };
                if row.claim.expires_at <= now_ms {
                    return terminal("expired", None, &kind_response("expired"));
                }
                match module_authority_tx(tx, project)? {
                    Some((generation, _)) if generation == row.claim.authority_generation => {}
                    _ => {
                        return terminal(
                            "authority_changed",
                            None,
                            &kind_response("authority_changed"),
                        );
                    }
                }
                match apply(coordinated, &row.claim)? {
                    LeaseCompletion::Stale => terminal("stale", None, &kind_response("stale")),
                    LeaseCompletion::Invalid { response } => {
                        terminal("invalid", None, &bound_response(&response))
                    }
                    LeaseCompletion::Applied { response_json } => {
                        let response_json = coordinated
                            .prepared
                            .borrow_mut()
                            .transaction_content("terminal_response", &response_json)
                            .map_err(redaction_error)?;
                        mark_terminal_tx(
                            tx,
                            kind,
                            project,
                            claim_id,
                            "applied",
                            Some(completion_id),
                            &response_json,
                            now_ms,
                        )?;
                        Ok(LeaseCompleteOutcome::Applied { response_json })
                    }
                }
            })()?;
            Ok(if tx.total_changes() > before {
                WriteDisposition::Applied(outcome)
            } else {
                WriteDisposition::Replay(outcome)
            })
        })
    }

    /// Terminally release a claim for controlled cancellation. Never touches
    /// the task; abandoning an already-terminal claim replays its terminal kind.
    pub(crate) fn abandon_task_lease(
        &self,
        kind: &TaskLeaseKind,
        project: &str,
        claim_id: &str,
        evaluator_instance: &str,
        evaluator_slot: i64,
        now_ms: i64,
    ) -> Result<LeaseAbandonOutcome, MemoryStoreError> {
        let mut write = PreparedWrite::new(LEDGER_FAMILY);
        write.domain_owner("project", project, claim_owner_key(kind, claim_id));
        for (field_id, value) in [
            ("project", project),
            ("claim_id", claim_id),
            ("evaluator_instance", evaluator_instance),
        ] {
            write.existing_identity(field_id, value)?;
        }
        self.with_prepared_note_conn_fenced(project, write, |coordinated| {
            let tx = coordinated.tx();
            let Some(row) = load_claim_tx(tx, kind, project, claim_id)? else {
                return Ok(WriteDisposition::Replay(LeaseAbandonOutcome::UnknownClaim));
            };
            if let Some(terminal_kind) = row.terminal_kind {
                return Ok(WriteDisposition::Replay(LeaseAbandonOutcome::Replayed {
                    kind: terminal_kind,
                }));
            }
            if row.claim.evaluator_instance != evaluator_instance
                || row.claim.evaluator_slot != evaluator_slot
            {
                return Ok(WriteDisposition::Replay(LeaseAbandonOutcome::Invalid));
            }
            mark_terminal_tx(
                tx,
                kind,
                project,
                claim_id,
                "abandoned",
                None,
                &kind_response("abandoned"),
                now_ms,
            )?;
            Ok(WriteDisposition::Applied(LeaseAbandonOutcome::Abandoned))
        })
    }
}
