//! Use outcomes record which decision objects a session was served at one frozen composition, so a later reader can name the sessions a finding reached and whether it was disputed after the serve.
//!
//! A serving record is one `use_outcomes` row per (project digest, harness, session, served-set digest) and one `use_outcome_decisions` row per served decision object. The record holds ids, commit sequences, and timestamps only; it is classified `sensitive` and `local_only` by DDL and is neither a registry object nor an evidence row, so no egress path reads it. The identity and the decision set never change after commit. Retirement sets `invalidated_commit_seq`; no row is deleted, so an admission row citing a record keeps resolving it.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use super::envelope::{PreparedIntent, commit_with_writer};
use super::local_file::{reserved_expiry_intent, reserved_expiry_key};
use super::redaction::identity;
use super::{CachedSql, Envelope, EventKind, KernelError, KernelStore, map_sqlite};

/// The retention class every serving record carries.
pub const USE_OUTCOME_RETENTION_CLASS: &str = "use_outcome_capture";
/// How long an uncited serving record is kept after it was recorded: 30 days, the harness-capture window.
pub const USE_OUTCOME_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
/// Serving records one expiry sweep retires per call, all in one commit.
pub const MAX_EXPIRED_USE_OUTCOMES_PER_CALL: usize = 64;

const SERVED_SET_DOMAIN: &[u8] = b"eidnara-served-set-v1\n";
const USE_OUTCOME_ID_DOMAIN: &[u8] = b"eidnara-use-outcome-v1\n";
const PAGE_KEY_DOMAIN: &[u8] = b"eidnara-use-outcome-expiry-page-v1\n";
/// The reserved producer suffix of expiry commits, also their actor.
const EXPIRY_PRODUCER: &str = "use-outcome-expiry";
const EXPIRY_CAUSE: &str = "use outcome retention expired";

/// Whether the served session was a primary session or a subagent session at the serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionRole {
    Primary,
    Subagent,
}

impl SessionRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Subagent => "subagent",
        }
    }

    /// The role a stored `session_role` names; `None` for any other text.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "primary" => Some(Self::Primary),
            "subagent" => Some(Self::Subagent),
            _ => None,
        }
    }
}

/// The served-set digest of `object_ids`: a domain-separated SHA-256 over the distinct ids in ascending order, each length-prefixed. Order and repetition in the input do not change it, so one id set has one digest; an empty set has a digest too.
pub fn served_set_digest<'a>(object_ids: impl IntoIterator<Item = &'a str>) -> String {
    let sorted: BTreeSet<&str> = object_ids.into_iter().collect();
    let mut hash = Sha256::new();
    hash.update(SERVED_SET_DOMAIN);
    for object_id in sorted {
        length_prefixed(&mut hash, object_id);
    }
    format!("{:x}", hash.finalize())
}

/// The id of the serving record for one identity tuple: a domain-separated SHA-256 over the four fields, each length-prefixed, so a writer and a citing reader compute it without a read.
pub fn use_outcome_id(
    project_digest: &str,
    harness: &str,
    session_id: &str,
    served_set_digest: &str,
) -> String {
    let mut hash = Sha256::new();
    hash.update(USE_OUTCOME_ID_DOMAIN);
    for field in [project_digest, harness, session_id, served_set_digest] {
        length_prefixed(&mut hash, field);
    }
    format!("{:x}", hash.finalize())
}

fn length_prefixed(hash: &mut Sha256, field: &str) {
    hash.update(field.len().to_string().as_bytes());
    hash.update(b":");
    hash.update(field.as_bytes());
    hash.update(b"\n");
}

/// One serve to record. `decision_object_ids` are the served registry object ids in strictly ascending order. `served_set_digest` is the writer's own [`served_set_digest`] over them; the insert recomputes it and refuses a mismatch, so the stored digest always names the stored set. `served_commit_seq` is the composition's `known_as_of`, a committed sequence before the recording commit; a composition read at sequence 0 has no commit to name and is not recordable. `composition_revision` is the daemon's informational revision, stored bit for bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseOutcomeSpec {
    pub project_digest: String,
    pub harness: String,
    pub session_id: String,
    pub served_set_digest: String,
    pub composition_revision: u64,
    pub served_commit_seq: i64,
    pub session_role: SessionRole,
    pub recorded_at: i64,
    pub decision_object_ids: Vec<String>,
}

/// One stored serving record without its decision rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseOutcomeServe {
    pub use_outcome_id: String,
    pub project_digest: String,
    pub harness: String,
    pub session_id: String,
    pub served_set_digest: String,
    pub composition_revision: u64,
    pub served_commit_seq: i64,
    pub session_role: SessionRole,
    pub recorded_at: i64,
    pub created_commit_seq: i64,
    /// Set when the expiry sweep retired the record.
    pub invalidated_commit_seq: Option<i64>,
    pub deleted_at: Option<i64>,
    pub last_transform_at: Option<i64>,
    pub retain_until: i64,
}

/// A serving record with its decision object ids in ascending order; an empty list is a serve of an empty composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseOutcomeRecord {
    pub serve: UseOutcomeServe,
    pub decision_object_ids: Vec<String>,
}

/// An unretired serve of one decision object and whether a `mark_disputed` or `contradict` on that object committed after the serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedSession {
    pub serve: UseOutcomeServe,
    pub disputed_after_serve: bool,
}

/// What one use-outcome expiry sweep did: `retired` records were retired by this call; `cited` expired records were kept because a committed admission row with a live subject cites them, counted among the rows the call read while filling its page. A cited record is kept, so a later call counts it again while its citation holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UseOutcomeExpiry {
    pub retired: usize,
    pub cited: usize,
}

const SERVE_COLUMNS: &str =
    "u.use_outcome_id,u.project_digest,u.harness,u.session_id,u.served_set_digest,
     u.composition_revision,u.served_commit_seq,u.session_role,u.recorded_at,u.created_commit_seq,
     u.invalidated_commit_seq,u.deleted_at,u.last_transform_at,u.retain_until";

/// SQL predicate over a `use_outcomes` row aliased `u`: a committed admission row cites the record and its subject decision object is live. The candidate scan and the per-record recheck evaluate this same text.
const USE_OUTCOME_CITED_SQL: &str = "EXISTS(SELECT 1 FROM admission_decisions a
               JOIN object_registry s ON s.object_id=a.subject_object_id
               WHERE a.use_outcome_id=u.use_outcome_id AND a.commit_seq IS NOT NULL
                 AND s.invalidated_commit_seq IS NULL)";

static SERVED_SET_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {SERVE_COLUMNS} FROM use_outcomes u
         WHERE u.project_digest=?1 AND u.harness=?2 AND u.session_id=?3
         ORDER BY u.created_commit_seq,u.use_outcome_id"
    )
});

/// The unretired serves of `?1` with the disputed-after-serve flag: a committed `mark_disputed` or `contradict` on the same object and lineage at a sequence strictly after the serve. The newest such sequence is read once for the object, and each serve compares its own `served_commit_seq` against it: some qualifying event is later than the serve exactly when the newest one is.
static SERVED_SESSIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH disputes(disputed_through) AS MATERIALIZED (
             SELECT (SELECT MAX(a.commit_seq) FROM admission_decisions a
                     WHERE a.subject_object_id=o.object_id
                       AND a.event_kind IN ('{disputed}','{contradict}')
                       AND {lineage}
                       AND a.commit_seq IS NOT NULL)
             FROM object_registry o WHERE o.object_id=?1)
         SELECT {SERVE_COLUMNS},
                COALESCE(u.served_commit_seq<disputes.disputed_through,0) AS disputed_after_serve
         FROM disputes
         JOIN use_outcome_decisions d ON d.object_id=?1
         JOIN use_outcomes u ON u.use_outcome_id=d.use_outcome_id
         WHERE u.invalidated_commit_seq IS NULL",
        disputed = EventKind::MarkDisputed.as_str(),
        contradict = EventKind::Contradict.as_str(),
        lineage = super::admission::same_lineage_as_registry_object("a"),
    )
});

/// Unretired records whose deadline is before `?1`, oldest deadline first, each with whether a citation keeps it. The scan reads cited rows ahead of the page, so its read work grows with the cited expired records while its writes stay bounded by the page.
static EXPIRED_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT u.use_outcome_id,{USE_OUTCOME_CITED_SQL} AS cited FROM use_outcomes u
         WHERE u.invalidated_commit_seq IS NULL AND u.retain_until<?1
         ORDER BY u.retain_until,u.use_outcome_id"
    )
});

/// Retires `?2` at commit `?1` when it is unretired and uncited under the writer; the recheck and the write are one statement.
static RETIRE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "UPDATE use_outcomes AS u SET invalidated_commit_seq=?1
         WHERE u.use_outcome_id=?2 AND u.invalidated_commit_seq IS NULL AND NOT {USE_OUTCOME_CITED_SQL}"
    )
});

static RECHECK_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT u.invalidated_commit_seq IS NULL AS live,{USE_OUTCOME_CITED_SQL} AS cited
         FROM use_outcomes u WHERE u.use_outcome_id=?1"
    )
});

impl UseOutcomeSpec {
    /// Refuses a malformed spec with `InvalidInput` and returns the record's `retain_until`. Checks run cheapest first: field shape, sequence and time range, the decision set and its digest, then the secret scan of every identity field.
    fn validated_retain_until(&self, commit_seq: i64) -> Result<i64, KernelError> {
        let identity_fields = [self.harness.as_str(), self.session_id.as_str()];
        let well_formed = crate::cas::is_artifact_digest(&self.project_digest)
            && crate::cas::is_artifact_digest(&self.served_set_digest)
            && identity_fields.iter().all(|field| !field.trim().is_empty());
        let in_range = self.recorded_at >= 0 && (1..commit_seq).contains(&self.served_commit_seq);
        let ids = &self.decision_object_ids;
        let set_agrees = ids.windows(2).all(|pair| pair[0] < pair[1])
            && ids.iter().all(|id| !id.trim().is_empty())
            && served_set_digest(ids.iter().map(String::as_str)) == self.served_set_digest;
        if !(well_formed && in_range && set_agrees) {
            return Err(KernelError::InvalidInput);
        }
        for field in identity_fields
            .into_iter()
            .chain(ids.iter().map(String::as_str))
        {
            identity(field)?;
        }
        self.recorded_at
            .checked_add(USE_OUTCOME_RETENTION_MS)
            .ok_or(KernelError::InvalidInput)
    }
}

impl Envelope<'_> {
    /// Records one serve: the serving record and one decision row per served object id, in ascending id order. Returns the record's id. The commit's receipt is the caller's intent, so an equal intent replays and a same-key intent under another digest is `Conflict`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` for a malformed spec (see [`UseOutcomeSpec`]), `NotFound` when a decision id names no registry object, `Conflict` when the identity is already recorded, and mapped storage errors otherwise.
    pub fn insert_use_outcome(&mut self, spec: UseOutcomeSpec) -> Result<String, KernelError> {
        self.guarded(|envelope| envelope.insert_use_outcome_inner(&spec))
    }

    fn insert_use_outcome_inner(&mut self, spec: &UseOutcomeSpec) -> Result<String, KernelError> {
        let retain_until = spec.validated_retain_until(self.commit_seq)?;
        let id = use_outcome_id(
            &spec.project_digest,
            &spec.harness,
            &spec.session_id,
            &spec.served_set_digest,
        );
        self.tx
            .execute_cached(
                "INSERT INTO use_outcomes(
                     use_outcome_id,project_digest,harness,session_id,served_set_digest,
                     decision_count,composition_revision,served_commit_seq,session_role,recorded_at,
                     created_commit_seq,retention_class,retain_until,sensitivity_class,
                     provider_egress_class
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'sensitive','local_only')",
                params![
                    id,
                    spec.project_digest,
                    spec.harness,
                    spec.session_id,
                    spec.served_set_digest,
                    i64::try_from(spec.decision_object_ids.len())
                        .map_err(|_| KernelError::InvalidInput)?,
                    spec.composition_revision.cast_signed(),
                    spec.served_commit_seq,
                    spec.session_role.as_str(),
                    spec.recorded_at,
                    self.commit_seq,
                    USE_OUTCOME_RETENTION_CLASS,
                    retain_until,
                ],
            )
            .map_err(map_sqlite)?;
        for object_id in &spec.decision_object_ids {
            self.tx
                .execute_cached(
                    "INSERT INTO use_outcome_decisions(use_outcome_id,object_id) VALUES (?1,?2)",
                    params![id, object_id],
                )
                .map_err(|error| {
                    // The record was written earlier in this transaction, so the reference a decision row can miss is its registry object. A duplicate identity fails the record's insert first; its recorded decision rows hold the same registry objects, which they keep from deletion.
                    if is_foreign_key_failure(&error) {
                        KernelError::NotFound
                    } else {
                        map_sqlite(error)
                    }
                })?;
        }
        Ok(id)
    }
}

/// The positions of [`SERVE_COLUMNS`] in one prepared statement, resolved by name once per statement, so each row reads its columns by position.
#[derive(Clone, Copy)]
struct ServeColumns {
    use_outcome_id: usize,
    project_digest: usize,
    harness: usize,
    session_id: usize,
    served_set_digest: usize,
    composition_revision: usize,
    served_commit_seq: usize,
    session_role: usize,
    recorded_at: usize,
    created_commit_seq: usize,
    invalidated_commit_seq: usize,
    deleted_at: usize,
    last_transform_at: usize,
    retain_until: usize,
}

impl ServeColumns {
    fn of(statement: &rusqlite::Statement<'_>) -> rusqlite::Result<Self> {
        let at = |name: &str| statement.column_index(name);
        Ok(Self {
            use_outcome_id: at("use_outcome_id")?,
            project_digest: at("project_digest")?,
            harness: at("harness")?,
            session_id: at("session_id")?,
            served_set_digest: at("served_set_digest")?,
            composition_revision: at("composition_revision")?,
            served_commit_seq: at("served_commit_seq")?,
            session_role: at("session_role")?,
            recorded_at: at("recorded_at")?,
            created_commit_seq: at("created_commit_seq")?,
            invalidated_commit_seq: at("invalidated_commit_seq")?,
            deleted_at: at("deleted_at")?,
            last_transform_at: at("last_transform_at")?,
            retain_until: at("retain_until")?,
        })
    }

    fn serve(self, row: &rusqlite::Row<'_>) -> rusqlite::Result<UseOutcomeServe> {
        let role: String = row.get(self.session_role)?;
        Ok(UseOutcomeServe {
            use_outcome_id: row.get(self.use_outcome_id)?,
            project_digest: row.get(self.project_digest)?,
            harness: row.get(self.harness)?,
            session_id: row.get(self.session_id)?,
            served_set_digest: row.get(self.served_set_digest)?,
            composition_revision: row
                .get::<_, i64>(self.composition_revision)?
                .cast_unsigned(),
            served_commit_seq: row.get(self.served_commit_seq)?,
            session_role: SessionRole::from_stored(&role).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    self.session_role,
                    rusqlite::types::Type::Text,
                    "unknown session role".into(),
                )
            })?,
            recorded_at: row.get(self.recorded_at)?,
            created_commit_seq: row.get(self.created_commit_seq)?,
            invalidated_commit_seq: row.get(self.invalidated_commit_seq)?,
            deleted_at: row.get(self.deleted_at)?,
            last_transform_at: row.get(self.last_transform_at)?,
            retain_until: row.get(self.retain_until)?,
        })
    }
}

fn decision_object_ids(tx: &Transaction<'_>, id: &str) -> Result<Vec<String>, KernelError> {
    let mut statement = tx
        .prepare_cached(
            "SELECT object_id FROM use_outcome_decisions WHERE use_outcome_id=?1 ORDER BY object_id",
        )
        .map_err(map_sqlite)?;
    statement
        .query_map([id], |row| row.get(0))
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)
}

impl KernelStore {
    /// Every serving record of one session, retired or not, oldest first, each with its decision ids; `None` when the session has no record. A record with no decision rows is a serve of an empty composition. The read issues one decision-row query per record, so its cost follows the distinct compositions the session was served within the retention window.
    ///
    /// # Errors
    ///
    /// Returns storage and lock errors, and `CorruptCanonicalRow` for a stored role outside the vocabulary.
    pub fn served_set(
        &self,
        project_digest: &str,
        harness: &str,
        session_id: &str,
    ) -> Result<Option<Vec<UseOutcomeRecord>>, KernelError> {
        let (_, records) = self.read_snapshot(0, |tx, _| {
            let mut statement = tx.prepare_cached(&SERVED_SET_SQL).map_err(map_sqlite)?;
            let columns = ServeColumns::of(&statement).map_err(map_sqlite)?;
            let serves = statement
                .query_map(params![project_digest, harness, session_id], |row| {
                    columns.serve(row)
                })
                .map_err(map_sqlite)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_read)?;
            serves
                .into_iter()
                .map(|serve| {
                    Ok(UseOutcomeRecord {
                        decision_object_ids: decision_object_ids(tx, &serve.use_outcome_id)?,
                        serve,
                    })
                })
                .collect::<Result<Vec<_>, KernelError>>()
        })?;
        Ok((!records.is_empty()).then_some(records))
    }

    /// The unretired serves of `object_id`, oldest first, each with `disputed_after_serve` derived at this read: a committed `mark_disputed` or `contradict` admission row on the object and its registry lineage at a commit sequence strictly greater than the serve's `served_commit_seq`. The flag is never stored and changes no visibility. The answer holds one entry per unretired serve of the object, so its size is bounded by the sessions served the object within the retention window plus cited records.
    ///
    /// # Errors
    ///
    /// Returns storage and lock errors, and `CorruptCanonicalRow` for a stored role outside the vocabulary.
    pub fn served_sessions_for(&self, object_id: &str) -> Result<Vec<ServedSession>, KernelError> {
        let (_, mut sessions) = self.read_snapshot(0, |tx, _| {
            let mut statement = tx
                .prepare_cached(&SERVED_SESSIONS_SQL)
                .map_err(map_sqlite)?;
            let columns = ServeColumns::of(&statement).map_err(map_sqlite)?;
            let disputed = statement
                .column_index("disputed_after_serve")
                .map_err(map_sqlite)?;
            statement
                .query_map([object_id], |row| {
                    Ok(ServedSession {
                        serve: columns.serve(row)?,
                        disputed_after_serve: row.get(disputed)?,
                    })
                })
                .map_err(map_sqlite)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_read)
        })?;
        // The query yields serves in decision-index order; the answer orders them by creating commit, then id.
        sessions.sort_unstable_by(|left, right| {
            (left.serve.created_commit_seq, &left.serve.use_outcome_id)
                .cmp(&(right.serve.created_commit_seq, &right.serve.use_outcome_id))
        });
        Ok(sessions)
    }

    /// Retires serving records whose `retain_until` is before `now` and that no committed admission row with a live subject cites: at most [`MAX_EXPIRED_USE_OUTCOMES_PER_CALL`] per call, oldest deadline first, in one commit. Each record is rechecked under the writer and gets its own reserved receipt keyed by `now` and its id, so a citation committed after selection keeps it and a record another sweep retired since selection is skipped. The commit's own receipt is keyed by `now` and a digest of the page's ids, so only a repeat of the same page at this cutoff replays; a replay retires nothing and reports the cited rows its scan read. A page that retires nothing commits nothing.
    ///
    /// # Errors
    ///
    /// Returns the first storage, lock, or fence error; nothing in its page is retired.
    pub fn expire_use_outcomes(&self, now: i64) -> Result<UseOutcomeExpiry, KernelError> {
        self.expire_use_outcomes_after_selection(now, || {})
    }

    fn expire_use_outcomes_after_selection(
        &self,
        now: i64,
        after_selection: impl FnOnce(),
    ) -> Result<UseOutcomeExpiry, KernelError> {
        let (page, sweep) = {
            let reader = self.lock_reader()?;
            expired_page(&reader, now)?
        };
        after_selection();
        if page.is_empty() {
            return Ok(sweep);
        }
        let mut tally = UseOutcomeExpiry::default();
        let mut empty_page = false;
        let mut writer = self.lock_writer()?;
        let outcome = commit_with_writer(
            &mut writer,
            self.lease_epoch(),
            reserved_expiry_intent(EXPIRY_PRODUCER, EXPIRY_CAUSE, now, &page_key(&page)),
            |envelope| {
                // Every record's receipt shares the producer, actor, and cause; only the key differs.
                let receipts = PreparedIntent::new(reserved_expiry_intent(
                    EXPIRY_PRODUCER,
                    EXPIRY_CAUSE,
                    now,
                    &page[0],
                ))?;
                for id in &page {
                    tally.count(retire_due_use_outcome(envelope, &receipts, now, id)?);
                }
                // A page that retires nothing rolls back, so it leaves no receipt a later call at this cutoff would replay.
                empty_page = tally.retired == 0;
                if empty_page {
                    return Err(KernelError::NotFound);
                }
                Ok(tally.retired.to_string())
            },
            || Ok(()),
        );
        match outcome {
            // Another sweep committed this exact page at this cutoff, so this call's commit did not run.
            Ok(receipt) if receipt.replayed => Ok(sweep),
            Ok(_) => Ok(sweep.merged(tally)),
            Err(KernelError::NotFound) if empty_page => Ok(sweep.merged(tally)),
            Err(error) => Err(error),
        }
    }
}

/// The page commit's key: a domain-separated digest over the page's ids in order, so only a repeat of this exact page at this cutoff replays its receipt.
fn page_key(page: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(PAGE_KEY_DOMAIN);
    for id in page {
        length_prefixed(&mut hash, id);
    }
    format!("page:{:x}", hash.finalize())
}

/// What the recheck under the writer found for one selected record.
enum Recheck {
    Retired,
    Cited,
    /// Retired by another sweep since selection.
    Gone,
}

impl UseOutcomeExpiry {
    fn merged(self, other: Self) -> Self {
        Self {
            retired: self.retired + other.retired,
            cited: self.cited + other.cited,
        }
    }

    fn count(&mut self, recheck: Recheck) {
        match recheck {
            Recheck::Retired => self.retired += 1,
            Recheck::Cited => self.cited += 1,
            Recheck::Gone => {}
        }
    }
}

fn is_foreign_key_failure(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
    )
}

fn map_read(error: rusqlite::Error) -> KernelError {
    match error {
        rusqlite::Error::FromSqlConversionFailure(..) => KernelError::CorruptCanonicalRow,
        other => map_sqlite(other),
    }
}

/// One statement, so one snapshot: up to one page of uncited expired record ids, oldest deadline first, and the count of cited expired records the scan read before the page filled.
fn expired_page(
    connection: &rusqlite::Connection,
    now: i64,
) -> Result<(Vec<String>, UseOutcomeExpiry), KernelError> {
    let mut statement = connection
        .prepare_cached(&EXPIRED_SQL)
        .map_err(map_sqlite)?;
    let mut rows = statement.query([now]).map_err(map_sqlite)?;
    let mut page = Vec::new();
    let mut sweep = UseOutcomeExpiry::default();
    while page.len() < MAX_EXPIRED_USE_OUTCOMES_PER_CALL {
        let Some(row) = rows.next().map_err(map_sqlite)? else {
            break;
        };
        if row.get::<_, bool>("cited").map_err(map_sqlite)? {
            sweep.cited += 1;
        } else {
            page.push(row.get("use_outcome_id").map_err(map_sqlite)?);
        }
    }
    Ok((page, sweep))
}

/// Retires record `id` when, under the writer, it is unretired and uncited, and records its reserved receipt keyed by `now` and the id. Its deadline is immutable and was before `now` at selection. One conditional update rechecks and retires the record, so a live, uncited record's update affects exactly that row; a record the update leaves in place is read again to report whether a citation kept it or another sweep retired it. `receipts` carries the page's reserved producer, actor, and cause.
fn retire_due_use_outcome(
    envelope: &mut Envelope<'_>,
    receipts: &PreparedIntent,
    now: i64,
    id: &str,
) -> Result<Recheck, KernelError> {
    let retired = envelope
        .tx
        .execute_cached(&RETIRE_SQL, params![envelope.commit_seq, id])
        .map_err(map_sqlite)?;
    if retired == 0 {
        let cited: Option<(bool, bool)> = envelope
            .tx
            .query_row_cached(&RECHECK_SQL, [id], |row| {
                Ok((row.get("live")?, row.get("cited")?))
            })
            .optional()
            .map_err(map_sqlite)?;
        return Ok(match cited {
            Some((true, true)) => Recheck::Cited,
            _ => Recheck::Gone,
        });
    }
    let (operation_key, request_digest) = reserved_expiry_key(now, id);
    let intent = receipts.rekeyed(&operation_key, request_digest)?;
    envelope.record_reserved_receipt(&intent, "retired")?;
    Ok(Recheck::Retired)
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::{
        EXPIRED_SQL, SERVED_SESSIONS_SQL, SERVED_SET_SQL, SessionRole, USE_OUTCOME_RETENTION_MS,
        UseOutcomeExpiry, UseOutcomeSpec,
    };
    use crate::{CommitIntent, DomainSpec, KernelStore, Sensitivity, served_set_digest};

    const NOW: i64 = USE_OUTCOME_RETENTION_MS + 1;

    fn intent(key: &str) -> CommitIntent {
        CommitIntent {
            producer: "test".to_string(),
            operation_key: key.to_string(),
            request_digest: "d".repeat(64),
            actor: "test".to_string(),
            cause: "test".to_string(),
        }
    }

    /// A store holding one expired serve of the domain object `decision` per session in `sessions`, and their ids in order.
    fn store_with_expired_serves(
        root: &std::path::Path,
        sessions: &[&str],
    ) -> (KernelStore, Vec<String>) {
        let store = KernelStore::open(root).unwrap();
        store
            .commit(intent("domain"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "decision".to_string(),
                    name: "domain".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "decision".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok(String::new())
            })
            .unwrap();
        let ids = sessions
            .iter()
            .map(|session| {
                let id = std::cell::RefCell::new(String::new());
                store
                    .commit(intent(session), |envelope| {
                        *id.borrow_mut() = envelope.insert_use_outcome(UseOutcomeSpec {
                            project_digest: "a".repeat(64),
                            harness: "opencode".to_string(),
                            session_id: (*session).to_string(),
                            served_set_digest: served_set_digest(["decision"]),
                            composition_revision: 1,
                            served_commit_seq: 1,
                            session_role: SessionRole::Subagent,
                            recorded_at: 0,
                            decision_object_ids: vec!["decision".to_string()],
                        })?;
                        Ok(String::new())
                    })
                    .unwrap();
                id.into_inner()
            })
            .collect();
        (store, ids)
    }

    fn cite(root: &std::path::Path, report: &str, use_outcome_id: &str) {
        Connection::open(root.join("kernel.sqlite"))
            .unwrap()
            .execute(
                "INSERT INTO admission_decisions(
                     admission_decision_id,subject_object_id,source_kind,source_id,
                     source_revision,source_class,taint_class,event_kind,maturity,
                     effective_maturity,disposition,visibility,outcome,sensitivity_class,
                     policy_revision,reason,commit_seq,decided_at,use_outcome_id
                 ) VALUES (?1,'decision','fixture','decision',1,'explicit_user',
                           'user_explicit','contradict','candidate','candidate',
                           'contradicted','automatic','admit','normal',1,'fixture',2,1,?2)",
                params![report, use_outcome_id],
            )
            .unwrap();
    }

    fn retire_the_cited_subject(root: &std::path::Path) {
        Connection::open(root.join("kernel.sqlite"))
            .unwrap()
            .execute(
                "UPDATE object_registry SET invalidated_commit_seq=2 WHERE object_id='decision'",
                [],
            )
            .unwrap();
    }

    fn retired(store: &KernelStore, session: &str) -> bool {
        store
            .served_set(&"a".repeat(64), "opencode", session)
            .unwrap()
            .unwrap()[0]
            .serve
            .invalidated_commit_seq
            .is_some()
    }

    /// A citation committed after the sweep chose its page and before the retiring commit is seen by the recheck under the writer: that record stays unretired and is counted cited, and the rest of the page retires in the same commit.
    #[test]
    fn a_record_cited_after_selection_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let (store, ids) = store_with_expired_serves(root.path(), &["kept", "due"]);
        let sweep = store
            .expire_use_outcomes_after_selection(NOW, || cite(root.path(), "report", &ids[0]))
            .unwrap();
        assert_eq!(
            sweep,
            UseOutcomeExpiry {
                retired: 1,
                cited: 1
            }
        );
        assert!(!retired(&store, "kept"));
        assert!(retired(&store, "due"));
        assert_eq!(
            store
                .served_set(&"a".repeat(64), "opencode", "kept")
                .unwrap()
                .unwrap()[0]
                .serve
                .session_role,
            SessionRole::Subagent
        );
    }

    /// A sweep racing another over the same page at the same cutoff replays the other's page receipt: its commit does not run, so a record the other sweep kept stays unretired even after its citation lapses, and a later call at the cutoff, whose page differs, retires it.
    #[test]
    fn a_page_another_sweep_committed_at_the_same_cutoff_replays() {
        let root = tempfile::tempdir().unwrap();
        let (store, ids) = store_with_expired_serves(root.path(), &["first", "second"]);
        let tip = std::cell::Cell::new(0);
        let sweep = store
            .expire_use_outcomes_after_selection(NOW, || {
                let other = store
                    .expire_use_outcomes_after_selection(NOW, || {
                        cite(root.path(), "report", &ids[1]);
                    })
                    .unwrap();
                assert_eq!(
                    other,
                    UseOutcomeExpiry {
                        retired: 1,
                        cited: 1
                    }
                );
                retire_the_cited_subject(root.path());
                tip.set(store.tip().unwrap());
            })
            .unwrap();
        assert_eq!(sweep, UseOutcomeExpiry::default());
        assert_eq!(
            store.tip().unwrap(),
            tip.get(),
            "the replayed page commits nothing"
        );
        assert!(!retired(&store, "second"), "the replay ran no recheck");
        assert_eq!(store.expire_use_outcomes(NOW).unwrap().retired, 1);
        assert!(retired(&store, "second"));
    }

    /// A replay reports the cited rows its own scan read: a record cited before selection stays out of the page and is counted, while the racing sweep commits the same page.
    #[test]
    fn a_replayed_page_reports_the_cited_rows_its_scan_read() {
        let root = tempfile::tempdir().unwrap();
        let (store, ids) = store_with_expired_serves(root.path(), &["cited", "due"]);
        cite(root.path(), "report", &ids[0]);
        let sweep = store
            .expire_use_outcomes_after_selection(NOW, || {
                assert_eq!(
                    store.expire_use_outcomes(NOW).unwrap(),
                    UseOutcomeExpiry {
                        retired: 1,
                        cited: 1
                    }
                );
            })
            .unwrap();
        assert_eq!(
            sweep,
            UseOutcomeExpiry {
                retired: 0,
                cited: 1
            }
        );
        assert!(!retired(&store, "cited"));
        assert!(retired(&store, "due"));
    }

    /// A record another sweep retired at a different cutoff after this sweep selected it is skipped, and a page that retires nothing commits nothing.
    #[test]
    fn a_record_retired_after_selection_at_another_cutoff_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        let (store, _) = store_with_expired_serves(root.path(), &["session"]);
        let tip = std::cell::Cell::new(0);
        let sweep = store
            .expire_use_outcomes_after_selection(NOW, || {
                assert_eq!(store.expire_use_outcomes(NOW + 1).unwrap().retired, 1);
                tip.set(store.tip().unwrap());
            })
            .unwrap();
        assert_eq!(sweep, UseOutcomeExpiry::default());
        assert_eq!(
            store.tip().unwrap(),
            tip.get(),
            "an empty page commits nothing"
        );
        assert!(retired(&store, "session"));
    }

    /// A record kept by a citation committed after selection retires at the same cutoff once the citation's subject is retired: the kept page left no receipt to replay.
    #[test]
    fn a_record_kept_at_a_cutoff_retires_at_that_cutoff_once_uncited() {
        let root = tempfile::tempdir().unwrap();
        let (store, ids) = store_with_expired_serves(root.path(), &["session"]);
        assert_eq!(
            store
                .expire_use_outcomes_after_selection(NOW, || cite(root.path(), "report", &ids[0]))
                .unwrap()
                .cited,
            1
        );
        retire_the_cited_subject(root.path());
        assert_eq!(store.expire_use_outcomes(NOW).unwrap().retired, 1);
        assert!(retired(&store, "session"));
    }

    /// The sweep scan reads the live-retention index in its order with no temporary sort and probes citations through the citing index; the by-session read seeks the identity index; the by-decision read starts from the decision-row index and probes disputes through the subject index.
    #[test]
    fn the_sweep_and_the_reads_use_their_indexes() {
        let root = tempfile::tempdir().unwrap();
        let _ = store_with_expired_serves(root.path(), &["session"]);
        let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
        let plan = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> String {
            let mut statement = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            statement
                .query_map(params, |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join("\n")
        };
        let expired = plan(&EXPIRED_SQL, &[&NOW]);
        assert!(
            expired.contains("idx_use_outcomes_live_retention"),
            "{expired}"
        );
        assert!(
            expired.contains("idx_admission_use_outcome_fk"),
            "{expired}"
        );
        assert!(!expired.contains("TEMP B-TREE"), "{expired}");
        let digest = "a".repeat(64);
        let by_session = plan(&SERVED_SET_SQL, &[&digest, &"opencode", &"session"]);
        assert!(
            by_session.contains("sqlite_autoindex_use_outcomes"),
            "{by_session}"
        );
        let by_decision = plan(&SERVED_SESSIONS_SQL, &[&"decision"]);
        assert!(
            by_decision.contains("idx_use_outcome_decisions_object_fk"),
            "{by_decision}"
        );
        assert!(
            by_decision.contains("idx_admission_subject_latest"),
            "{by_decision}"
        );
    }
}
