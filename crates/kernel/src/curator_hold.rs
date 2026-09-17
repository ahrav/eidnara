//! Curator holds: capture pins that keep selected evidence bytes alive for one review job's execution and, after a proposal is persisted, for its review window.
//!
//! A hold is a `capture_pins` row of kind [`CURATOR_EXECUTION_HOLD_KIND`] or [`CURATOR_REVIEW_HOLD_KIND`] whose `owner_id` is the tuple `project_digest / kernel incarnation / MemoryStore incarnation / subject / generation`, unit-separated with the project digest first so one project's held bytes are a prefix range. Validity is the pin's own row: unreleased, not purge-degraded, and before `expires_at`; the process lease epoch is not part of it, because a hold outlives the worker that acquired it. A hold protects bytes only. It grants no read eligibility, and every disclosure revalidates policy separately.
//!
//! Release authority is kind-specific. An execution hold ends through the atomic review-hold replacement in [`KernelStore::transfer_execution_to_review`], a trusted terminal receipt through [`KernelStore::release_execution_hold`], or fixed expiry in capture-pin maintenance. A review hold ends through Kernel settlement or rejection in [`KernelStore::release_review_hold`], or review expiry. Every release names the exact binding, so a worker holding a prior generation cannot release a successor's hold. Purge degradation overrides both kinds through the existing capture-pin path.

use std::collections::BTreeMap;
use std::ops::Range;

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::backup::release_capture_pin_in_tx;
use super::cas::ArtifactHandle;
use super::envelope::{Sensitivity, check_fence};
use super::redaction::identity;
use super::{CachedSql, KernelError, KernelStore, current_time_ms, map_sqlite};

pub const CURATOR_EXECUTION_HOLD_KIND: &str = "curator_execution";
pub const CURATOR_REVIEW_HOLD_KIND: &str = "curator_review";
/// `evidence_meta.retention_class` of evidence a Curator run captured for itself; such rows must carry a finite `retain_until`.
pub const CURATOR_CAPTURE_RETENTION_CLASS: &str = "curator_capture";
/// Review expiry bound for a selected proposal, measured from result creation.
pub const REVIEW_EXPIRY_MAX_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
pub const MAX_CURATOR_HELD_BACKING_BYTES_PER_PROJECT: u64 = 64 * 1024 * 1024;
pub const MAX_CURATOR_HELD_BACKING_BYTES_PER_HOST: u64 = 256 * 1024 * 1024;
/// Distinct retained buffer bytes one run may hold in memory.
pub const MAX_RUN_BUFFER_BYTES: u64 = 16 * 1024 * 1024;
/// Evidence references one hold may carry: the subject, starting references, and every inspected artifact.
pub const MAX_CURATOR_HOLD_REFERENCES: usize = 512;
/// Active Curator holds per project and per host, matching the pending-work bounds.
pub const MAX_ACTIVE_CURATOR_HOLDS_PER_PROJECT: usize = 64;
pub const MAX_ACTIVE_CURATOR_HOLDS_PER_HOST: usize = 256;

/// Held-backing bounds an acquisition is admitted against; production uses the constants above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackingQuota {
    project_bytes: u64,
    host_bytes: u64,
}

const PRODUCTION_QUOTA: BackingQuota = BackingQuota {
    project_bytes: MAX_CURATOR_HELD_BACKING_BYTES_PER_PROJECT,
    host_bytes: MAX_CURATOR_HELD_BACKING_BYTES_PER_HOST,
};

const OWNER_SEPARATOR: char = '\u{1f}';
const OWNER_SEPARATOR_SUCCESSOR: char = '\u{20}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuratorHoldKind {
    Execution,
    Review,
}

impl CuratorHoldKind {
    fn pin_kind(self) -> &'static str {
        match self {
            Self::Execution => CURATOR_EXECUTION_HOLD_KIND,
            Self::Review => CURATOR_REVIEW_HOLD_KIND,
        }
    }
}

/// Who owns a hold. `subject` is the job id for an execution hold and the provisional result candidate id for a review hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorHoldBinding {
    pub project_digest: String,
    pub kernel_incarnation: String,
    pub memstore_incarnation: String,
    pub subject: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorHold {
    pub hold_id: String,
    pub kind: CuratorHoldKind,
    pub expires_at: i64,
    pub references: usize,
    /// Distinct artifact bytes the hold's references name.
    pub backing_bytes: u64,
}

/// Facts about one held evidence row, read under the hold and returned for the caller's own policy checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldEvidence {
    pub evidence_id: String,
    pub artifact_digest: String,
    pub byte_length: u64,
    pub sensitivity: Sensitivity,
    pub provider_egress_class: String,
    pub retention_class: String,
    pub retain_until: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CuratorHoldRefusal {
    #[error("curator hold request is malformed")]
    InvalidRequest,
    #[error("curator hold names another kernel incarnation")]
    IncarnationMismatch,
    #[error("an evidence id is unknown, invalidated, or purged")]
    UnavailableEvidence,
    #[error("the hold would carry more than {MAX_CURATOR_HOLD_REFERENCES} references")]
    TooManyReferences,
    #[error("the project already has {MAX_ACTIVE_CURATOR_HOLDS_PER_PROJECT} active curator holds")]
    ProjectHoldLimit,
    #[error("the host already has {MAX_ACTIVE_CURATOR_HOLDS_PER_HOST} active curator holds")]
    HostHoldLimit,
    #[error("held backing would exceed the project bound")]
    ProjectBackingExhausted,
    #[error("held backing would exceed the host bound")]
    HostBackingExhausted,
    #[error("no hold of this kind and binding has the id")]
    Missing,
    #[error("the hold was released")]
    Released,
    #[error("the hold expired")]
    Expired,
    #[error("a purge degraded the hold")]
    PurgeDegraded,
    #[error("an evidence id is not covered by the hold")]
    NotCovered,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CuratorHoldError {
    #[error(transparent)]
    Refused(#[from] CuratorHoldRefusal),
    #[error(transparent)]
    Store(KernelError),
}

impl From<KernelError> for CuratorHoldError {
    fn from(error: KernelError) -> Self {
        Self::Store(error)
    }
}

fn sqlite(error: rusqlite::Error) -> CuratorHoldError {
    CuratorHoldError::Store(map_sqlite(error))
}

fn corrupt<T>(_: T) -> CuratorHoldError {
    CuratorHoldError::Store(KernelError::CorruptCanonicalRow)
}

impl CuratorHoldBinding {
    fn validate(&self) -> Result<(), CuratorHoldRefusal> {
        for field in [
            &self.project_digest,
            &self.kernel_incarnation,
            &self.memstore_incarnation,
            &self.subject,
        ] {
            if field.is_empty()
                || field.len() > 256
                || field.contains(OWNER_SEPARATOR)
                || identity(field).is_err()
            {
                return Err(CuratorHoldRefusal::InvalidRequest);
            }
        }
        Ok(())
    }

    fn owner_id(&self) -> String {
        format!(
            "{}{sep}{}{sep}{}{sep}{}{sep}{}",
            self.project_digest,
            self.kernel_incarnation,
            self.memstore_incarnation,
            self.subject,
            self.generation,
            sep = OWNER_SEPARATOR
        )
    }

    fn project_prefix(&self) -> String {
        format!("{}{OWNER_SEPARATOR}", self.project_digest)
    }
}

struct StoredHold {
    kind: CuratorHoldKind,
    expires_at: i64,
    released: bool,
    purge_degraded: bool,
}

impl KernelStore {
    /// Pins `evidence_ids` for one job until `expires_at`, the job's run cutoff, after validating every id, the reference count, the active-hold counts, and the distinct backing bytes the project and host would hold. Nothing is written when any check fails.
    pub fn acquire_execution_hold(
        &self,
        binding: &CuratorHoldBinding,
        evidence_ids: &[String],
        expires_at: i64,
    ) -> Result<CuratorHold, CuratorHoldError> {
        self.acquire_hold(
            CuratorHoldKind::Execution,
            binding,
            evidence_ids,
            expires_at,
            PRODUCTION_QUOTA,
        )
    }

    /// Acquires an execution hold against smaller backing bounds so quota refusal can be reached with small fixtures.
    #[cfg(feature = "test-support")]
    pub fn acquire_execution_hold_with_quota_for_test(
        &self,
        binding: &CuratorHoldBinding,
        evidence_ids: &[String],
        expires_at: i64,
        project_bytes: u64,
        host_bytes: u64,
    ) -> Result<CuratorHold, CuratorHoldError> {
        self.acquire_hold(
            CuratorHoldKind::Execution,
            binding,
            evidence_ids,
            expires_at,
            BackingQuota {
                project_bytes,
                host_bytes,
            },
        )
    }

    /// Adds `evidence_ids` to a live hold as the dependency union grows. The expiry never moves; the same bounds apply to the whole hold; a replayed extension adds nothing twice.
    pub fn extend_curator_hold(
        &self,
        hold_id: &str,
        kind: CuratorHoldKind,
        binding: &CuratorHoldBinding,
        evidence_ids: &[String],
    ) -> Result<CuratorHold, CuratorHoldError> {
        binding.validate()?;
        let hold_id = identity(hold_id).map_err(|_| CuratorHoldRefusal::InvalidRequest)?;
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        self.check_incarnation(&tx, binding)?;
        let now = current_time_ms();
        let stored = load_valid_hold(&tx, &hold_id, kind, binding, now)?;
        add_references(&tx, &hold_id, stored.expires_at, evidence_ids)?;
        let hold = admit_totals(
            &tx,
            &hold_id,
            kind,
            binding,
            stored.expires_at,
            PRODUCTION_QUOTA,
        )?;
        tx.commit().map_err(sqlite)?;
        Ok(hold)
    }

    /// In one envelope: acquire the review hold over the execution hold's still-live references, move still-future `retain_until` of Curator-captured evidence to the review expiry, then release the execution hold. `review_expires_at` is at most [`REVIEW_EXPIRY_MAX_MS`] after `result_created_at`. Expired references are never extended and ownership never changes.
    pub fn transfer_execution_to_review(
        &self,
        execution_hold_id: &str,
        execution: &CuratorHoldBinding,
        review: &CuratorHoldBinding,
        result_created_at: i64,
        review_expires_at: i64,
    ) -> Result<CuratorHold, CuratorHoldError> {
        execution.validate()?;
        review.validate()?;
        let execution_hold_id =
            identity(execution_hold_id).map_err(|_| CuratorHoldRefusal::InvalidRequest)?;
        let now = current_time_ms();
        let review_ceiling = result_created_at
            .checked_add(REVIEW_EXPIRY_MAX_MS)
            .ok_or(CuratorHoldRefusal::InvalidRequest)?;
        if result_created_at < 0
            || result_created_at > now
            || review_expires_at <= now
            || review_expires_at > review_ceiling
            || execution.project_digest != review.project_digest
            || execution.kernel_incarnation != review.kernel_incarnation
            || execution.memstore_incarnation != review.memstore_incarnation
        {
            return Err(CuratorHoldRefusal::InvalidRequest.into());
        }
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        self.check_incarnation(&tx, review)?;
        load_valid_hold(
            &tx,
            &execution_hold_id,
            CuratorHoldKind::Execution,
            execution,
            now,
        )?;
        let review_hold_id = insert_pin(
            &tx,
            CuratorHoldKind::Review,
            review,
            review_expires_at,
            self.lease_epoch(),
        )?;
        // New reference rows: the schema forbids re-parenting an active reference, and the review hold outlives the execution hold.
        tx.execute_cached(
            "INSERT INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
             SELECT ?1,r.evidence_id,?2 FROM capture_pin_refs r
             JOIN evidence_meta e ON e.evidence_id=r.evidence_id
             WHERE r.capture_pin_id=?3 AND r.released_at IS NULL
               AND e.invalidated_commit_seq IS NULL
             ORDER BY r.evidence_id",
            params![review_hold_id, review_expires_at, execution_hold_id],
        )
        .map_err(sqlite)?;
        // Only Curator-captured evidence whose acquisition reference is still live moves to the review expiry.
        tx.execute_cached(
            "UPDATE evidence_meta SET retain_until=?1
             WHERE retention_class=?2 AND retain_until IS NOT NULL AND retain_until>?3
               AND invalidated_commit_seq IS NULL
               AND evidence_id IN (SELECT evidence_id FROM capture_pin_refs WHERE capture_pin_id=?4)",
            params![
                review_expires_at,
                CURATOR_CAPTURE_RETENTION_CLASS,
                now,
                review_hold_id
            ],
        )
        .map_err(sqlite)?;
        let hold = admit_totals(
            &tx,
            &review_hold_id,
            CuratorHoldKind::Review,
            review,
            review_expires_at,
            PRODUCTION_QUOTA,
        )?;
        if !release_capture_pin_in_tx(&tx, &execution_hold_id, now)? {
            return Err(CuratorHoldRefusal::Released.into());
        }
        tx.commit().map_err(sqlite)?;
        Ok(hold)
    }

    /// Releases an execution hold on a trusted terminal receipt for its job and generation.
    pub fn release_execution_hold(
        &self,
        hold_id: &str,
        binding: &CuratorHoldBinding,
    ) -> Result<(), CuratorHoldError> {
        self.release_hold(hold_id, CuratorHoldKind::Execution, binding)
    }

    /// Releases a review hold after Kernel settlement or rejection of its proposal.
    pub fn release_review_hold(
        &self,
        hold_id: &str,
        binding: &CuratorHoldBinding,
    ) -> Result<(), CuratorHoldError> {
        self.release_hold(hold_id, CuratorHoldKind::Review, binding)
    }

    /// Batch guard before a dependent MemoryStore operation or disclosure: every id must be covered by the live hold and name live, unpurged evidence. Returns facts for the caller's policy checks; the hold itself grants no eligibility.
    pub fn validate_held_evidence(
        &self,
        hold_id: &str,
        kind: CuratorHoldKind,
        binding: &CuratorHoldBinding,
        evidence_ids: &[String],
        now: i64,
    ) -> Result<Vec<HeldEvidence>, CuratorHoldError> {
        binding.validate()?;
        if now < 0 || evidence_ids.len() > MAX_CURATOR_HOLD_REFERENCES {
            return Err(CuratorHoldRefusal::InvalidRequest.into());
        }
        let hold_id = identity(hold_id).map_err(|_| CuratorHoldRefusal::InvalidRequest)?;
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite)?;
        self.check_incarnation(&tx, binding)?;
        load_valid_hold(&tx, &hold_id, kind, binding, now.max(current_time_ms()))?;
        let mut facts = Vec::with_capacity(evidence_ids.len());
        for evidence_id in evidence_ids {
            let evidence_id =
                identity(evidence_id).map_err(|_| CuratorHoldRefusal::InvalidRequest)?;
            let covered: bool = tx
                .query_row_cached(
                    "SELECT EXISTS(SELECT 1 FROM capture_pin_refs
                     WHERE capture_pin_id=?1 AND evidence_id=?2 AND released_at IS NULL)",
                    params![hold_id, evidence_id],
                    |row| row.get(0),
                )
                .map_err(sqlite)?;
            if !covered {
                return Err(CuratorHoldRefusal::NotCovered.into());
            }
            facts.push(load_live_evidence(&tx, &evidence_id)?);
        }
        Ok(facts)
    }

    fn acquire_hold(
        &self,
        kind: CuratorHoldKind,
        binding: &CuratorHoldBinding,
        evidence_ids: &[String],
        expires_at: i64,
        quota: BackingQuota,
    ) -> Result<CuratorHold, CuratorHoldError> {
        binding.validate()?;
        let now = current_time_ms();
        if expires_at <= now || evidence_ids.is_empty() {
            return Err(CuratorHoldRefusal::InvalidRequest.into());
        }
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        self.check_incarnation(&tx, binding)?;
        let hold_id = insert_pin(&tx, kind, binding, expires_at, self.lease_epoch())?;
        add_references(&tx, &hold_id, expires_at, evidence_ids)?;
        let hold = admit_totals(&tx, &hold_id, kind, binding, expires_at, quota)?;
        tx.commit().map_err(sqlite)?;
        Ok(hold)
    }

    fn release_hold(
        &self,
        hold_id: &str,
        kind: CuratorHoldKind,
        binding: &CuratorHoldBinding,
    ) -> Result<(), CuratorHoldError> {
        binding.validate()?;
        let hold_id = identity(hold_id).map_err(|_| CuratorHoldRefusal::InvalidRequest)?;
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        let now = current_time_ms();
        let stored = load_hold(&tx, &hold_id, kind, binding)?.ok_or(CuratorHoldRefusal::Missing)?;
        if stored.released {
            return Err(CuratorHoldRefusal::Released.into());
        }
        if !release_capture_pin_in_tx(&tx, &hold_id, now)? {
            return Err(CuratorHoldRefusal::Released.into());
        }
        tx.commit().map_err(sqlite)?;
        Ok(())
    }

    fn check_incarnation(
        &self,
        tx: &Transaction<'_>,
        binding: &CuratorHoldBinding,
    ) -> Result<(), CuratorHoldError> {
        let incarnation = super::open::database_incarnation_id_via(tx)?;
        if incarnation != binding.kernel_incarnation {
            return Err(CuratorHoldRefusal::IncarnationMismatch.into());
        }
        Ok(())
    }
}

fn insert_pin(
    tx: &Transaction<'_>,
    kind: CuratorHoldKind,
    binding: &CuratorHoldBinding,
    expires_at: i64,
    lease_epoch: u64,
) -> Result<String, CuratorHoldError> {
    let prefix = binding.project_prefix();
    let project_active: i64 = tx
        .query_row_cached(
            "SELECT COUNT(*) FROM capture_pins
             WHERE pin_kind IN (?1,?2) AND released_at IS NULL
               AND owner_id>=?3 AND owner_id<?4",
            params![
                CURATOR_EXECUTION_HOLD_KIND,
                CURATOR_REVIEW_HOLD_KIND,
                prefix,
                format!("{}{OWNER_SEPARATOR_SUCCESSOR}", binding.project_digest)
            ],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    if usize::try_from(project_active).map_err(corrupt)? >= MAX_ACTIVE_CURATOR_HOLDS_PER_PROJECT {
        return Err(CuratorHoldRefusal::ProjectHoldLimit.into());
    }
    let host_active: i64 = tx
        .query_row_cached(
            "SELECT COUNT(*) FROM capture_pins WHERE pin_kind IN (?1,?2) AND released_at IS NULL",
            params![CURATOR_EXECUTION_HOLD_KIND, CURATOR_REVIEW_HOLD_KIND],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    if usize::try_from(host_active).map_err(corrupt)? >= MAX_ACTIVE_CURATOR_HOLDS_PER_HOST {
        return Err(CuratorHoldRefusal::HostHoldLimit.into());
    }
    let snapshot: i64 = tx
        .query_row_cached(
            "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    let hold_id: String = tx
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(sqlite)?;
    let epoch = i64::try_from(lease_epoch).map_err(|_| KernelError::InvalidInput)?;
    tx.execute_cached(
        "INSERT INTO capture_pins(
             capture_pin_id,pin_kind,owner_id,commit_seq,lease_epoch,writer_epoch,created_at,expires_at
         ) VALUES (?1,?2,?3,?4,?5,?5,?6,?7)",
        params![
            hold_id,
            kind.pin_kind(),
            binding.owner_id(),
            snapshot,
            epoch,
            current_time_ms(),
            expires_at
        ],
    )
    .map_err(sqlite)?;
    Ok(hold_id)
}

/// Every id must name live, untombstoned evidence; `INSERT OR IGNORE` makes a replayed reference a no-op.
fn add_references(
    tx: &Transaction<'_>,
    hold_id: &str,
    expires_at: i64,
    evidence_ids: &[String],
) -> Result<(), CuratorHoldError> {
    if evidence_ids.len() > MAX_CURATOR_HOLD_REFERENCES {
        return Err(CuratorHoldRefusal::TooManyReferences.into());
    }
    for evidence_id in evidence_ids {
        let evidence_id = identity(evidence_id).map_err(|_| CuratorHoldRefusal::InvalidRequest)?;
        load_live_evidence(tx, &evidence_id)?;
        tx.execute_cached(
            "INSERT OR IGNORE INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
             VALUES (?1,?2,?3)",
            params![hold_id, evidence_id, expires_at],
        )
        .map_err(sqlite)?;
    }
    Ok(())
}

fn load_live_evidence(
    tx: &Transaction<'_>,
    evidence_id: &str,
) -> Result<HeldEvidence, CuratorHoldError> {
    let evidence = tx
        .query_row_cached(
            "SELECT e.artifact_digest,e.byte_length,e.sensitivity_class,e.provider_egress_class,
                    e.retention_class,e.retain_until
             FROM evidence_meta e
             WHERE e.evidence_id=?1 AND e.invalidated_commit_seq IS NULL
               AND NOT EXISTS(SELECT 1 FROM artifact_purge_tombstones t
                              WHERE t.artifact_digest=e.artifact_digest)",
            [evidence_id],
            |row| {
                Ok(HeldEvidence {
                    evidence_id: evidence_id.to_string(),
                    artifact_digest: row.get(0)?,
                    byte_length: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    sensitivity: Sensitivity::from_stored(&row.get::<_, String>(2)?),
                    provider_egress_class: row.get(3)?,
                    retention_class: row.get(4)?,
                    retain_until: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(sqlite)?;
    evidence.ok_or_else(|| CuratorHoldRefusal::UnavailableEvidence.into())
}

/// Whole-hold reference count and the distinct backing the project and host hold across every active Curator pin, checked after the rows exist and before the transaction commits.
fn admit_totals(
    tx: &Transaction<'_>,
    hold_id: &str,
    kind: CuratorHoldKind,
    binding: &CuratorHoldBinding,
    expires_at: i64,
    quota: BackingQuota,
) -> Result<CuratorHold, CuratorHoldError> {
    let (references, backing_bytes): (i64, Option<i64>) = tx
        .query_row_cached(
            "SELECT COUNT(*),(SELECT SUM(byte_length) FROM (
                 SELECT DISTINCT e.artifact_digest,e.byte_length FROM capture_pin_refs r
                 JOIN evidence_meta e ON e.evidence_id=r.evidence_id
                 WHERE r.capture_pin_id=?1 AND r.released_at IS NULL))
             FROM capture_pin_refs WHERE capture_pin_id=?1 AND released_at IS NULL",
            [hold_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite)?;
    let references = usize::try_from(references).map_err(corrupt)?;
    if references > MAX_CURATOR_HOLD_REFERENCES {
        return Err(CuratorHoldRefusal::TooManyReferences.into());
    }
    let held = |project: Option<&str>| -> Result<u64, CuratorHoldError> {
        let (lower, upper) = match project {
            Some(digest) => (
                format!("{digest}{OWNER_SEPARATOR}"),
                format!("{digest}{OWNER_SEPARATOR_SUCCESSOR}"),
            ),
            None => (String::new(), char::MAX.to_string()),
        };
        let total: Option<i64> = tx
            .query_row_cached(
                "SELECT SUM(byte_length) FROM (
                     SELECT DISTINCT e.artifact_digest,e.byte_length FROM capture_pin_refs r
                     JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
                     JOIN evidence_meta e ON e.evidence_id=r.evidence_id
                     WHERE p.pin_kind IN (?1,?2) AND p.released_at IS NULL AND r.released_at IS NULL
                       AND p.owner_id>=?3 AND p.owner_id<?4)",
                params![
                    CURATOR_EXECUTION_HOLD_KIND,
                    CURATOR_REVIEW_HOLD_KIND,
                    lower,
                    upper
                ],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        u64::try_from(total.unwrap_or(0)).map_err(corrupt)
    };
    if held(Some(&binding.project_digest))? > quota.project_bytes {
        return Err(CuratorHoldRefusal::ProjectBackingExhausted.into());
    }
    if held(None)? > quota.host_bytes {
        return Err(CuratorHoldRefusal::HostBackingExhausted.into());
    }
    Ok(CuratorHold {
        hold_id: hold_id.to_string(),
        kind,
        expires_at,
        references,
        backing_bytes: u64::try_from(backing_bytes.unwrap_or(0)).map_err(corrupt)?,
    })
}

fn load_hold(
    tx: &Transaction<'_>,
    hold_id: &str,
    kind: CuratorHoldKind,
    binding: &CuratorHoldBinding,
) -> Result<Option<StoredHold>, CuratorHoldError> {
    tx.query_row_cached(
        "SELECT expires_at,released_at,purge_degraded_at FROM capture_pins
         WHERE capture_pin_id=?1 AND pin_kind=?2 AND owner_id=?3",
        params![hold_id, kind.pin_kind(), binding.owner_id()],
        |row| {
            Ok(StoredHold {
                kind,
                expires_at: row.get::<_, Option<i64>>(0)?.unwrap_or(i64::MAX),
                released: row.get::<_, Option<i64>>(1)?.is_some(),
                purge_degraded: row.get::<_, Option<i64>>(2)?.is_some(),
            })
        },
    )
    .optional()
    .map_err(sqlite)
}

fn load_valid_hold(
    tx: &Transaction<'_>,
    hold_id: &str,
    kind: CuratorHoldKind,
    binding: &CuratorHoldBinding,
    now: i64,
) -> Result<StoredHold, CuratorHoldError> {
    let stored = load_hold(tx, hold_id, kind, binding)?.ok_or(CuratorHoldRefusal::Missing)?;
    if stored.released {
        return Err(CuratorHoldRefusal::Released.into());
    }
    if stored.purge_degraded {
        return Err(CuratorHoldRefusal::PurgeDegraded.into());
    }
    if now >= stored.expires_at {
        return Err(CuratorHoldRefusal::Expired.into());
    }
    debug_assert_eq!(stored.kind, kind);
    Ok(stored)
}

/// Index of one loaded artifact inside a [`RunBufferMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferIndex(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RunBufferRefusal {
    #[error("the artifact does not fit the run's remaining buffer capacity")]
    CapacityExhausted,
    #[error("the artifact could not be read or failed verification")]
    Unreadable,
    #[error("the range lies outside the buffer")]
    RangeOutOfBounds,
}

/// Append-only map from artifact digest to one owned buffer, charged against a fixed run capacity before loading. Each distinct artifact loads once; repeated ranges reuse it; nothing is evicted or reloaded.
#[derive(Debug)]
pub struct RunBufferMap {
    capacity: u64,
    used: u64,
    buffers: Vec<Vec<u8>>,
    by_digest: BTreeMap<String, BufferIndex>,
}

impl RunBufferMap {
    /// `capacity` is clamped to [`MAX_RUN_BUFFER_BYTES`].
    pub fn new(capacity: u64) -> Self {
        Self {
            capacity: capacity.min(MAX_RUN_BUFFER_BYTES),
            used: 0,
            buffers: Vec::new(),
            by_digest: BTreeMap::new(),
        }
    }

    pub fn remaining(&self) -> u64 {
        self.capacity - self.used
    }

    pub fn loaded(&self) -> usize {
        self.buffers.len()
    }

    /// Charges `held.byte_length` before reading; a read whose length disagrees with the evidence row is refused and nothing is retained.
    pub fn load(
        &mut self,
        store: &KernelStore,
        held: &HeldEvidence,
    ) -> Result<BufferIndex, RunBufferRefusal> {
        if let Some(index) = self.by_digest.get(&held.artifact_digest) {
            return Ok(*index);
        }
        if held.byte_length > self.remaining() {
            return Err(RunBufferRefusal::CapacityExhausted);
        }
        let bytes = store
            .read_artifact(&ArtifactHandle {
                digest: held.artifact_digest.clone(),
                evidence_id: held.evidence_id.clone(),
            })
            .map_err(|_| RunBufferRefusal::Unreadable)?;
        if u64::try_from(bytes.len()).ok() != Some(held.byte_length) {
            return Err(RunBufferRefusal::Unreadable);
        }
        self.used += held.byte_length;
        let index = BufferIndex(self.buffers.len());
        self.buffers.push(bytes);
        self.by_digest.insert(held.artifact_digest.clone(), index);
        Ok(index)
    }

    /// Borrows `range` of a loaded buffer for rendering; the borrow ends with the caller's use.
    pub fn slice(&self, index: BufferIndex, range: Range<u64>) -> Result<&[u8], RunBufferRefusal> {
        let buffer = self
            .buffers
            .get(index.0)
            .ok_or(RunBufferRefusal::RangeOutOfBounds)?;
        let start = usize::try_from(range.start).map_err(|_| RunBufferRefusal::RangeOutOfBounds)?;
        let end = usize::try_from(range.end).map_err(|_| RunBufferRefusal::RangeOutOfBounds)?;
        buffer
            .get(start..end)
            .ok_or(RunBufferRefusal::RangeOutOfBounds)
    }
}
