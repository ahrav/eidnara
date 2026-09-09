//! The Dreamer receipt ledger. It stores one durable receipt for each
//! unattended request and one row for each model attempt under a receipt.
//!
//! A receipt is `ABSENT` until `begin` writes it `IN_PROGRESS(generation)`
//! before any model dispatch; `complete` moves it to `COMPLETE(generation)`
//! with the terminal response. Every transition after `begin` carries the
//! generation the caller holds and is guarded by a row predicate on key,
//! generation, and state, so a predecessor that lost a takeover affects zero
//! rows and learns it is fenced instead of writing over the successor. Receipts
//! are retained for the database incarnation; nothing prunes them.

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use context_core::claim_operation::is_lower_hex;

use crate::{
    DurableWriteFamily, JsonScanPolicy, MemoryStore, MemoryStoreError, PreparedWrite,
    WriteDisposition, active_scan_owner_key,
};

pub use context_core::claim_operation::{
    DREAMER_REQUEST_ENCODING_VERSION, compute_dreamer_request_digest,
};

/// The project-scoped request identity a receipt is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DreamerReceiptKey<'a> {
    pub project: &'a str,
    pub producer: &'a str,
    pub operation_key: &'a str,
}

/// What a receipt binds its request to: the store incarnation and authority
/// generation the request ran under, the digest over its effect-defining
/// inputs, and the client identity that issued it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamerReceiptBinding {
    pub database_incarnation_id: String,
    pub authority_generation: u64,
    pub request_digest: String,
    pub ledger_session: String,
    pub command_id: String,
}

/// How an attempt or a whole request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamerTerminalKind {
    Complete,
    Failed,
    Cancelled,
    /// A dispatch marker exists but the run's outcome can no longer be learned.
    Unknown,
}

impl DreamerTerminalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "complete" => Some(Self::Complete),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// A receipt's durable state; `ABSENT` is the absence of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamerReceiptState {
    InProgress {
        generation: u64,
    },
    Complete {
        generation: u64,
        terminal_kind: DreamerTerminalKind,
        result_json: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamerReceipt {
    pub binding: DreamerReceiptBinding,
    pub state: DreamerReceiptState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// The answer to `begin`. An `IN_PROGRESS` receipt is reported as such and is
/// never a terminal response; the caller decides whether it may take over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamerBeginOutcome {
    /// The receipt was absent and now exists at generation 1.
    Begun {
        generation: u64,
    },
    InProgress {
        generation: u64,
    },
    Complete {
        generation: u64,
        terminal_kind: DreamerTerminalKind,
        result_json: String,
    },
    /// The identity is stored under another request digest; nothing is written.
    DigestConflict {
        stored_digest: String,
    },
    /// The identity is stored under another incarnation or authority generation.
    BindingMismatch {
        field: &'static str,
        expected: String,
        found: String,
    },
}

/// Whether a guarded transition matched its row predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamerTransition {
    Applied,
    /// The predicate matched no row: another generation owns the receipt, or
    /// the state already moved. Nothing was written.
    Fenced,
}

/// The identities an attempt row records before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamerAttemptSpec<'a> {
    pub attempt_index: u32,
    pub model: &'a str,
    pub prompt_template_version: u32,
    pub system_prompt_hash: &'a str,
    pub schema_version: u32,
    pub child_session: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamerAttempt {
    pub generation: u64,
    pub attempt_index: u32,
    pub model: String,
    pub prompt_template_version: u32,
    pub system_prompt_hash: String,
    pub schema_version: u32,
    pub child_session: String,
    pub dispatched_at_ms: i64,
    pub run_handle: Option<String>,
    pub terminal_kind: Option<DreamerTerminalKind>,
    pub terminal_at_ms: Option<i64>,
    pub session_released_at_ms: Option<i64>,
}

const RECEIPT_COLUMNS: &str = "database_incarnation_id, authority_generation, request_digest,
     ledger_session, command_id, state, generation, terminal_kind, result_json,
     created_at_ms, updated_at_ms";

fn receipt_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DreamerReceipt> {
    let state: String = row.get(5)?;
    let generation = generation_from_column(6, row.get::<_, i64>(6)?)?;
    let terminal_kind: Option<String> = row.get(7)?;
    let result_json: Option<String> = row.get(8)?;
    let state = match (state.as_str(), terminal_kind, result_json) {
        ("in_progress", None, None) => DreamerReceiptState::InProgress { generation },
        ("complete", Some(kind), Some(result_json)) => DreamerReceiptState::Complete {
            generation,
            terminal_kind: parse_terminal_kind(7, kind)?,
            result_json,
        },
        (other, _, _) => {
            return Err(rusqlite::Error::InvalidColumnType(
                5,
                other.to_string(),
                rusqlite::types::Type::Text,
            ));
        }
    };
    Ok(DreamerReceipt {
        binding: DreamerReceiptBinding {
            database_incarnation_id: row.get(0)?,
            authority_generation: generation_from_column(1, row.get::<_, i64>(1)?)?,
            request_digest: row.get(2)?,
            ledger_session: row.get(3)?,
            command_id: row.get(4)?,
        },
        state,
        created_at_ms: row.get(9)?,
        updated_at_ms: row.get(10)?,
    })
}

fn parse_terminal_kind(column: usize, kind: String) -> rusqlite::Result<DreamerTerminalKind> {
    DreamerTerminalKind::parse(&kind).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(column, kind, rusqlite::types::Type::Text)
    })
}

fn generation_from_column(column: usize, value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| {
        rusqlite::Error::InvalidColumnType(
            column,
            value.to_string(),
            rusqlite::types::Type::Integer,
        )
    })
}

fn generation_param(generation: u64) -> Result<i64, MemoryStoreError> {
    i64::try_from(generation)
        .map_err(|_| MemoryStoreError::Serde("generation exceeds the storable range".to_string()))
}

fn validate_key(key: DreamerReceiptKey<'_>) -> Result<(), MemoryStoreError> {
    if key.project.is_empty() {
        return Err(MemoryStoreError::Serde(
            "dreamer receipt project must not be empty".to_string(),
        ));
    }
    for (name, value) in [
        ("producer", key.producer),
        ("operation_key", key.operation_key),
    ] {
        if value.is_empty() || value.len() > 256 {
            return Err(MemoryStoreError::Serde(format!(
                "dreamer receipt {name} must be 1 to 256 bytes"
            )));
        }
    }
    Ok(())
}

/// The schema's CHECK constraints bound every other field; the digest's
/// character set is the one rule SQLite cannot express.
fn validate_binding(binding: &DreamerReceiptBinding) -> Result<(), MemoryStoreError> {
    if !is_lower_hex(&binding.request_digest, 64) {
        return Err(MemoryStoreError::Serde(
            "dreamer receipt request digest must be 64 lowercase hex characters".to_string(),
        ));
    }
    generation_param(binding.authority_generation).map(|_| ())
}

/// The write coordinator's owner for every write under one receipt key.
fn receipt_write(key: DreamerReceiptKey<'_>) -> PreparedWrite {
    let mut write = PreparedWrite::new(DurableWriteFamily::CommandLedgers);
    write.domain_owner(
        "project",
        key.project,
        active_scan_owner_key(&["dreamer", key.producer, key.operation_key]),
    );
    write
}

fn disposition(changed: usize) -> WriteDisposition<DreamerTransition> {
    if changed == 0 {
        WriteDisposition::Replay(DreamerTransition::Fenced)
    } else {
        WriteDisposition::Applied(DreamerTransition::Applied)
    }
}

impl MemoryStore {
    /// Moves the request from `ABSENT` to `IN_PROGRESS(1)`, or reports the
    /// receipt it already has. Nothing is dispatched inside; the caller
    /// dispatches only after `Begun` or a successful takeover.
    pub fn begin_dreamer_receipt(
        &self,
        key: DreamerReceiptKey<'_>,
        binding: &DreamerReceiptBinding,
        now_ms: i64,
    ) -> Result<DreamerBeginOutcome, MemoryStoreError> {
        validate_key(key)?;
        validate_binding(binding)?;
        let authority_generation = generation_param(binding.authority_generation)?;
        let mut write = receipt_write(key);
        write.existing_identity("project", key.project)?;
        write.identity("producer", key.producer)?;
        write.identity("operation_key", key.operation_key)?;
        write.identity("ledger_session", &binding.ledger_session)?;
        write.identity("command_id", &binding.command_id)?;
        write.execute(&self.inner, |coordinated| {
            let tx = coordinated.tx();
            let existing = tx
                .query_row(
                    &format!(
                        "SELECT {RECEIPT_COLUMNS} FROM dreamer_receipts
                          WHERE project = ?1 AND producer = ?2 AND operation_key = ?3"
                    ),
                    params![key.project, key.producer, key.operation_key],
                    receipt_from_row,
                )
                .optional()?;
            if let Some(existing) = existing {
                let outcome = if existing.binding.request_digest != binding.request_digest {
                    DreamerBeginOutcome::DigestConflict {
                        stored_digest: existing.binding.request_digest,
                    }
                } else if existing.binding.database_incarnation_id
                    != binding.database_incarnation_id
                {
                    DreamerBeginOutcome::BindingMismatch {
                        field: "database_incarnation_id",
                        expected: existing.binding.database_incarnation_id,
                        found: binding.database_incarnation_id.clone(),
                    }
                } else if existing.binding.authority_generation != binding.authority_generation {
                    DreamerBeginOutcome::BindingMismatch {
                        field: "authority_generation",
                        expected: existing.binding.authority_generation.to_string(),
                        found: binding.authority_generation.to_string(),
                    }
                } else {
                    match existing.state {
                        DreamerReceiptState::InProgress { generation } => {
                            DreamerBeginOutcome::InProgress { generation }
                        }
                        DreamerReceiptState::Complete {
                            generation,
                            terminal_kind,
                            result_json,
                        } => DreamerBeginOutcome::Complete {
                            generation,
                            terminal_kind,
                            result_json,
                        },
                    }
                };
                return Ok(WriteDisposition::Replay(outcome));
            }
            tx.execute(
                "INSERT INTO dreamer_receipts (
                     project, producer, operation_key, database_incarnation_id,
                     authority_generation, request_encoding_version, request_digest,
                     ledger_session, command_id, state, generation, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'in_progress', 1, ?10, ?10)",
                params![
                    key.project,
                    key.producer,
                    key.operation_key,
                    binding.database_incarnation_id,
                    authority_generation,
                    DREAMER_REQUEST_ENCODING_VERSION,
                    binding.request_digest,
                    binding.ledger_session,
                    binding.command_id,
                    now_ms,
                ],
            )?;
            Ok(WriteDisposition::Applied(DreamerBeginOutcome::Begun {
                generation: 1,
            }))
        })
    }

    /// Adopts an `IN_PROGRESS` receipt at `predecessor_generation` under
    /// `predecessor_generation + 1`. The caller must already have proven the
    /// predecessor's child session quiescent or deleted; this only moves the
    /// fence. A receipt that is complete or already past
    /// `predecessor_generation` is `Fenced`.
    pub fn take_over_dreamer_receipt(
        &self,
        key: DreamerReceiptKey<'_>,
        predecessor_generation: u64,
        now_ms: i64,
    ) -> Result<DreamerTransition, MemoryStoreError> {
        let predecessor = generation_param(predecessor_generation)?;
        let successor = generation_param(predecessor_generation.saturating_add(1))?;
        self.guarded_transition(
            key,
            |_| Ok(()),
            "UPDATE dreamer_receipts SET generation = ?4, updated_at_ms = ?5
              WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                AND state = 'in_progress' AND generation = ?6",
            params![successor, now_ms, predecessor],
        )
    }

    /// Records the dispatch marker for one model attempt before the model is
    /// called. The row exists only while the receipt is `IN_PROGRESS` at
    /// `generation`, so a fenced predecessor cannot add attempts.
    pub fn begin_dreamer_attempt(
        &self,
        key: DreamerReceiptKey<'_>,
        generation: u64,
        spec: &DreamerAttemptSpec<'_>,
        now_ms: i64,
    ) -> Result<DreamerTransition, MemoryStoreError> {
        let generation = generation_param(generation)?;
        self.guarded_transition(
            key,
            |write| {
                write.identity("model", spec.model)?;
                write.identity("child_session", spec.child_session)?;
                write.identity("system_prompt_hash", spec.system_prompt_hash)?;
                Ok(())
            },
            "INSERT INTO dreamer_attempts (
                 project, producer, operation_key, generation, attempt_index, model,
                 prompt_template_version, system_prompt_hash, schema_version,
                 child_session, dispatched_at_ms
             )
             SELECT project, producer, operation_key, generation, ?4, ?5, ?6, ?7, ?8, ?9, ?10
               FROM dreamer_receipts
              WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                AND state = 'in_progress' AND generation = ?11",
            params![
                spec.attempt_index,
                spec.model,
                spec.prompt_template_version,
                spec.system_prompt_hash,
                spec.schema_version,
                spec.child_session,
                now_ms,
                generation,
            ],
        )
    }

    /// Records the run handle the model runtime returned for an attempt.
    pub fn record_dreamer_run_handle(
        &self,
        key: DreamerReceiptKey<'_>,
        generation: u64,
        attempt_index: u32,
        run_handle: &str,
    ) -> Result<DreamerTransition, MemoryStoreError> {
        let generation = generation_param(generation)?;
        self.guarded_transition(
            key,
            |write| write.identity("run_handle", run_handle).map(|_| ()),
            "UPDATE dreamer_attempts SET run_handle = ?6
              WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                AND generation = ?4 AND attempt_index = ?5 AND terminal_kind IS NULL",
            params![generation, attempt_index, run_handle],
        )
    }

    /// Ends one attempt. An attempt ends once; a second terminal write is `Fenced`.
    pub fn finish_dreamer_attempt(
        &self,
        key: DreamerReceiptKey<'_>,
        generation: u64,
        attempt_index: u32,
        terminal_kind: DreamerTerminalKind,
        now_ms: i64,
    ) -> Result<DreamerTransition, MemoryStoreError> {
        let generation = generation_param(generation)?;
        self.guarded_transition(
            key,
            |_| Ok(()),
            "UPDATE dreamer_attempts SET terminal_kind = ?6, terminal_at_ms = ?7
              WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                AND generation = ?4 AND attempt_index = ?5 AND terminal_kind IS NULL",
            params![generation, attempt_index, terminal_kind.as_str(), now_ms],
        )
    }

    /// Moves the receipt from `IN_PROGRESS(generation)` to `COMPLETE(generation)`
    /// with the terminal response the request replays from then on. A failed
    /// run completes the receipt too, under a failed terminal kind, so a retry
    /// replays the failure instead of dispatching again.
    pub fn complete_dreamer_receipt(
        &self,
        key: DreamerReceiptKey<'_>,
        generation: u64,
        terminal_kind: DreamerTerminalKind,
        result_json: &str,
        now_ms: i64,
    ) -> Result<DreamerTransition, MemoryStoreError> {
        validate_key(key)?;
        let generation = generation_param(generation)?;
        let mut write = receipt_write(key);
        let result_json = write.json_content(
            "result_json",
            result_json,
            JsonScanPolicy::DurableRejectProtected,
        )?;
        write.execute(&self.inner, |coordinated| {
            let changed = coordinated.tx().execute(
                "UPDATE dreamer_receipts
                    SET state = 'complete', terminal_kind = ?5, result_json = ?6, updated_at_ms = ?7
                  WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                    AND state = 'in_progress' AND generation = ?4",
                params![
                    key.project,
                    key.producer,
                    key.operation_key,
                    generation,
                    terminal_kind.as_str(),
                    result_json,
                    now_ms
                ],
            )?;
            Ok(disposition(changed))
        })
    }

    /// Claims the right to delete an attempt's child session. The release is
    /// recorded once per attempt and only for the generation that owns it, so a
    /// fenced predecessor never deletes a successor's session.
    pub fn release_dreamer_attempt_session(
        &self,
        key: DreamerReceiptKey<'_>,
        generation: u64,
        attempt_index: u32,
        now_ms: i64,
    ) -> Result<DreamerTransition, MemoryStoreError> {
        let generation = generation_param(generation)?;
        self.guarded_transition(
            key,
            |_| Ok(()),
            "UPDATE dreamer_attempts SET session_released_at_ms = ?6
              WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                AND generation = ?4 AND attempt_index = ?5
                AND session_released_at_ms IS NULL
                AND EXISTS (
                    SELECT 1 FROM dreamer_receipts r
                     WHERE r.project = ?1 AND r.producer = ?2 AND r.operation_key = ?3
                       AND r.generation = ?4
                )",
            params![generation, attempt_index, now_ms],
        )
    }

    /// Runs one row-predicate-guarded statement under the receipt's write
    /// owner. The key binds `?1`..`?3`; `rest` binds the statement's remaining
    /// parameters in order. Zero affected rows is `Fenced` and commits nothing.
    fn guarded_transition(
        &self,
        key: DreamerReceiptKey<'_>,
        prepare: impl FnOnce(&mut PreparedWrite) -> Result<(), MemoryStoreError>,
        sql: &str,
        rest: &[&dyn rusqlite::ToSql],
    ) -> Result<DreamerTransition, MemoryStoreError> {
        validate_key(key)?;
        let mut write = receipt_write(key);
        prepare(&mut write)?;
        write.execute(&self.inner, |coordinated| {
            let mut bound: Vec<&dyn rusqlite::ToSql> =
                vec![&key.project, &key.producer, &key.operation_key];
            bound.extend_from_slice(rest);
            let changed = coordinated.tx().execute(sql, bound.as_slice())?;
            Ok(disposition(changed))
        })
    }

    /// The receipt under `key`, or `None` when the request is `ABSENT`.
    pub fn lookup_dreamer_receipt(
        &self,
        key: DreamerReceiptKey<'_>,
    ) -> Result<Option<DreamerReceipt>, MemoryStoreError> {
        validate_key(key)?;
        self.inner
            .with_conn(|conn| {
                conn.query_row(
                    &format!(
                        "SELECT {RECEIPT_COLUMNS} FROM dreamer_receipts
                          WHERE project = ?1 AND producer = ?2 AND operation_key = ?3"
                    ),
                    params![key.project, key.producer, key.operation_key],
                    receipt_from_row,
                )
                .optional()
            })
            .map_err(Into::into)
    }

    /// Every attempt recorded under `key`, oldest generation and index first.
    pub fn list_dreamer_attempts(
        &self,
        key: DreamerReceiptKey<'_>,
    ) -> Result<Vec<DreamerAttempt>, MemoryStoreError> {
        validate_key(key)?;
        self.inner
            .with_conn(|conn| {
                let mut statement = conn.prepare_cached(
                    "SELECT generation, attempt_index, model, prompt_template_version,
                            system_prompt_hash, schema_version, child_session, dispatched_at_ms,
                            run_handle, terminal_kind, terminal_at_ms, session_released_at_ms
                       FROM dreamer_attempts
                      WHERE project = ?1 AND producer = ?2 AND operation_key = ?3
                      ORDER BY generation, attempt_index",
                )?;
                let rows = statement.query_map(
                    params![key.project, key.producer, key.operation_key],
                    |row| {
                        let terminal_kind: Option<String> = row.get(9)?;
                        Ok(DreamerAttempt {
                            generation: generation_from_column(0, row.get::<_, i64>(0)?)?,
                            attempt_index: row.get::<_, u32>(1)?,
                            model: row.get(2)?,
                            prompt_template_version: row.get(3)?,
                            system_prompt_hash: row.get(4)?,
                            schema_version: row.get(5)?,
                            child_session: row.get(6)?,
                            dispatched_at_ms: row.get(7)?,
                            run_handle: row.get(8)?,
                            terminal_kind: terminal_kind
                                .map(|kind| parse_terminal_kind(9, kind))
                                .transpose()?,
                            terminal_at_ms: row.get(10)?,
                            session_released_at_ms: row.get(11)?,
                        })
                    },
                )?;
                rows.collect()
            })
            .map_err(Into::into)
    }

    /// Attempts dispatched for `project` at or after `since_ms`: the per-project
    /// count a budget refuses on. Every attempt counts, whatever its outcome,
    /// because each one was a model dispatch.
    pub fn count_dreamer_attempts(
        &self,
        project: &str,
        since_ms: i64,
    ) -> Result<u64, MemoryStoreError> {
        self.inner
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM dreamer_attempts
                      WHERE project = ?1 AND dispatched_at_ms >= ?2",
                    params![project, since_ms],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|count| u64::try_from(count).unwrap_or(0))
            .map_err(Into::into)
    }
}

/// Digest over the effect-defining inputs of one Dreamer request. Canonical
/// JSON encoding sorts object keys by byte order, so two maps with the same
/// entries in different insertion order digest identically.
pub fn dreamer_request_digest(inputs: &Value) -> Result<String, MemoryStoreError> {
    compute_dreamer_request_digest(inputs)
        .map_err(|error| MemoryStoreError::Serde(error.to_string()))
}
