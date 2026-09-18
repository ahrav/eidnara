//! Curator review jobs and frozen Memory Classifier selections.
//!
//! A job row is a permanent receipt for the store incarnation. It is `Reserved` when a producer reserves its causal identity and worst-case allowance, `Ready` once the producer activates it with reference-only input, and `Terminal` with one content-free outcome afterwards; nothing deletes a row or reopens a terminal one. `(project, causal_identity)` deduplicates identical inputs across firings: the identity is a digest of the review target plus the fingerprint of question template, signals, required evidence with availability, and policy versions, so a new contradiction, newly available evidence, or a policy change permits exactly one new job at an unchanged target, while unrelated writes, clocks, and firing identifiers change nothing.
//!
//! Capacity counts `Reserved` and `Ready` rows: 64 per project, 256 per host. Metadata quota counts each row's receipt charge plus allowances for non-terminal rows and frozen pages: 64 MiB per project, 256 MiB per host. Terminal jobs and pages release allowances and drop inputs or references but retain receipt charges, which bounds cumulative admitted work for the incarnation. Exhaustion refuses new work without deletion. Queue deadlines are fixed at reservation.
//!
//! Every write below composes with a caller's own fenced transaction through the `*_in_tx` primitives, so a producer can activate a job or enqueue a frozen page together with its own progress and commit both or neither.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use storage::GuardedConn;

use context_core::canonical_json::is_lower_hex;
use context_core::redaction::reject_transaction_secret_text;

use crate::{
    DurableWriteFamily, MemoryStore, MemoryStoreError, PreparedWrite, WriteDisposition,
    active_scan_owner_key, retire_active_scan_domain_owner,
};

pub const MAX_PENDING_CURATOR_JOBS_PER_PROJECT: usize = 64;
pub const MAX_PENDING_CURATOR_JOBS_PER_HOST: usize = 256;
pub const MAX_FROZEN_SELECTIONS_PER_PROJECT: usize = 1;
pub const MAX_FROZEN_SELECTIONS_PER_HOST: usize = 32;
pub const MAX_SELECTION_REFERENCES: usize = 8;
pub const MAX_CURATOR_STARTING_REFERENCES: usize = 8;
pub const MAX_CAUSAL_SIGNALS: usize = 8;
pub const MAX_REQUIRED_EVIDENCE: usize = 16;
pub const MAX_CAUSAL_POLICY_VERSIONS: usize = 16;
pub const MAX_CURATOR_JOB_INPUT_BYTES: usize = 8 * 1024;
/// Serialized bound of one frozen page. Eight references at typical identity lengths fit; eight references at every identity bound do not, and are refused rather than truncated.
pub const MAX_FROZEN_PAGE_BYTES: usize = 64 * 1024;
const MAX_TARGET_JSON_BYTES: usize = 1024;
/// Queue deadline and frozen-selection deadline, measured from reservation.
pub const CURATOR_QUEUE_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
pub const MAX_CURATOR_METADATA_BYTES_PER_PROJECT: u64 = 64 * 1024 * 1024;
pub const MAX_CURATOR_METADATA_BYTES_PER_HOST: u64 = 256 * 1024 * 1024;
/// Permanent receipt charge every admitted job keeps for the store incarnation: the job row, its receipt, up to four attempt markers, and the scan-audit rows their caller text leaves, all sized at their byte bounds. Worst case: the job row (project 256, firing id 256, target 1024, template 256, digests 128) about 2 KiB; the receipt (project, authority store 256, claim id 200, candidate 256, digest 64, incarnations 64) about 1.3 KiB; four markers (project, provider 128, model 256, credential 256, digests 128) about 1 KiB each; the primary-key index entry each row repeats, about 2.5 KiB together; and the scan audit, roughly 2.6 KiB per dispatch (one batch, one domain owner, four field scans and owner copies with their indexes), about 1 KiB for the selection, and about 3 KiB for the reservation and activation identities, some 14 KiB in all. Terminal rows keep it, so it also bounds lifetime admissions per store incarnation: `MAX_CURATOR_METADATA_BYTES_PER_PROJECT / CURATOR_RECEIPT_CHARGE_BYTES` (2,048) jobs per project and four times that per host before reservation refuses for good.
pub const CURATOR_RECEIPT_CHARGE_BYTES: u64 = 32 * 1024;
/// Permanent receipt charge each frozen page keeps once terminal: the compact row (project, slot id, and selection attempt at 256 bytes each, plus state and timestamps) and the primary-key index entry that repeats those keys, about 1.7 KiB at the bounds.
pub const FROZEN_PAGE_RECEIPT_CHARGE_BYTES: u64 = 4096;
/// Worst-case temporary allowance a reservation prepays for its input, holds, manifest, and attempt metadata; released when the job is terminal.
pub const CURATOR_JOB_ALLOWANCE_BYTES: u64 = 32 * 1024;
/// Allowance a `frozen` page holds for its serialized references, sized to [`MAX_FROZEN_PAGE_BYTES`]; terminal pages hold none.
pub const FROZEN_SELECTION_ALLOWANCE_BYTES: u64 = 64 * 1024;

const MAX_IDENTITY_BYTES: usize = 256;

/// What a review job investigates: a sealed staged subject by immutable digest, or a canonical memory at one revision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewTarget {
    StagedSubject {
        kernel_incarnation: String,
        candidate_id: String,
        payload_digest: String,
    },
    Memory {
        object_id: String,
        source_revision: i64,
    },
}

/// One piece of required evidence and whether the producer could reach it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceAvailability {
    pub evidence_id: String,
    pub available: bool,
}

/// Every input that makes one review causally distinct. Lists are sorted and deduplicated before hashing so producer ordering does not create identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalInputs {
    pub target: ReviewTarget,
    pub question_template: String,
    pub signals: Vec<String>,
    pub required_evidence: Vec<EvidenceAvailability>,
    pub policy_versions: BTreeMap<String, String>,
}

/// The producer firing that reserved a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerBinding {
    pub producer: String,
    pub firing_id: String,
    pub ordinal: u64,
}

/// Reference-only job input: the subject, at most eight starting references, and the fixed question template. Serialized size is bounded by [`MAX_CURATOR_JOB_INPUT_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuratorJobInput {
    pub subject: ReviewTarget,
    pub starting_references: Vec<String>,
    pub question_template: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuratorJobOutcome {
    Expired,
    Nonadmitted,
    Failed,
    Unknown,
    Completed,
    Abstained,
}

impl CuratorJobOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Nonadmitted => "nonadmitted",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Completed => "completed",
            Self::Abstained => "abstained",
        }
    }

    const ALL: [Self; 6] = [
        Self::Expired,
        Self::Nonadmitted,
        Self::Failed,
        Self::Unknown,
        Self::Completed,
        Self::Abstained,
    ];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuratorJobState {
    Reserved,
    Ready(CuratorJobInput),
    Terminal(CuratorJobOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorJob {
    pub project: String,
    pub causal_identity: String,
    pub producer: ProducerBinding,
    pub target: ReviewTarget,
    /// The reserved template; activation input must carry the same one.
    pub question_template: String,
    pub input_fingerprint: String,
    pub state: CuratorJobState,
    pub queue_deadline_ms: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveOutcome {
    /// A new row exists; the caller now stages or captures against its allowance.
    Reserved(CuratorJob),
    /// Identical causal inputs already have a row; its state and outcome stand.
    Existing(CuratorJob),
}

/// Bounded references a Memory Classifier slot selected for review, frozen before enqueue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenSelectionPage {
    pub references: Vec<CausalInputs>,
    /// Keyset continuation the slot advances to only when the page is enqueued.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrozenSelectionState {
    Frozen,
    Enqueued,
    Expired,
    FailedSlot,
}

impl FrozenSelectionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Frozen => "frozen",
            Self::Enqueued => "enqueued",
            Self::Expired => "expired",
            Self::FailedSlot => "failed_slot",
        }
    }

    const ALL: [Self; 4] = [
        Self::Frozen,
        Self::Enqueued,
        Self::Expired,
        Self::FailedSlot,
    ];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenSelection {
    pub project: String,
    pub slot_id: String,
    pub selection_attempt: String,
    /// References and continuation cursor for `Frozen` pages and enqueue results; terminal rows omit both.
    pub page: Option<FrozenSelectionPage>,
    pub state: FrozenSelectionState,
    pub selection_deadline_ms: i64,
    pub created_at_ms: i64,
}

/// Capacity and quota headroom for one project, read so operators and producers can refuse before reserving. Every bound a reservation or freeze checks is reported: project and host pending jobs, project and host frozen pages, and both metadata quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CuratorHeadroom {
    pub pending_jobs: usize,
    pub host_pending_jobs: usize,
    pub frozen_pages: usize,
    pub host_frozen_pages: usize,
    pub project_metadata_bytes: u64,
    pub host_metadata_bytes: u64,
    pub project_metadata_remaining: u64,
    pub host_metadata_remaining: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CuratorJobRefusal {
    #[error("the request is malformed or over a bound")]
    InvalidRequest,
    #[error("the project already has {MAX_PENDING_CURATOR_JOBS_PER_PROJECT} pending jobs")]
    ProjectCapacity,
    #[error("the host already has {MAX_PENDING_CURATOR_JOBS_PER_HOST} pending jobs")]
    HostCapacity,
    #[error("the project already has a frozen selection")]
    ProjectSelectionCapacity,
    #[error("the host already has {MAX_FROZEN_SELECTIONS_PER_HOST} frozen selections")]
    HostSelectionCapacity,
    #[error("the permanent metadata quota is exhausted for this store incarnation")]
    MetadataQuota,
    #[error("no row has this identity")]
    Missing,
    #[error("the job is not reserved")]
    NotReserved,
    #[error("the job or selection is already terminal")]
    Terminal,
    #[error("the queue deadline has passed")]
    Expired,
    #[error("the producer binding differs from the reservation")]
    ProducerMismatch,
    #[error("the scheduler slot's lease was not the caller's to complete")]
    SlotConflict,
    #[error("the frozen page exceeds its serialized bound")]
    PageTooLarge,
}

impl CuratorJobRefusal {
    #[cfg(any(test, feature = "test-support"))]
    pub const ALL: [Self; 13] = [
        Self::InvalidRequest,
        Self::ProjectCapacity,
        Self::HostCapacity,
        Self::ProjectSelectionCapacity,
        Self::HostSelectionCapacity,
        Self::MetadataQuota,
        Self::Missing,
        Self::NotReserved,
        Self::Terminal,
        Self::Expired,
        Self::ProducerMismatch,
        Self::SlotConflict,
        Self::PageTooLarge,
    ];
}

#[derive(Debug, thiserror::Error)]
pub enum CuratorJobError {
    #[error(transparent)]
    Refused(#[from] CuratorJobRefusal),
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
}

impl From<rusqlite::Error> for CuratorJobError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(MemoryStoreError::Store(storage::StoreError::Backend(
            error.to_string(),
        )))
    }
}

pub(crate) fn refuse(refusal: CuratorJobRefusal) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(refusal))
}

/// Reads the refusal a transaction body raised through `refuse` before the storage layer flattens the error to text.
pub(crate) fn refusal_of(error: &rusqlite::Error) -> Option<CuratorJobRefusal> {
    match error {
        rusqlite::Error::ToSqlConversionFailure(inner) => {
            inner.downcast_ref::<CuratorJobRefusal>().copied()
        }
        _ => None,
    }
}

/// A project is an identity: 1 to 256 bytes, refused as a store error at the public surface so it never reaches a row.
pub(crate) fn check_project(project: &str) -> Result<(), MemoryStoreError> {
    check_identity(project)
        .map_err(|_| MemoryStoreError::Serde("curator project must be 1 to 256 bytes".to_string()))
}

fn check_identity(value: &str) -> Result<(), CuratorJobRefusal> {
    if value.is_empty() || value.len() > MAX_IDENTITY_BYTES {
        return Err(CuratorJobRefusal::InvalidRequest);
    }
    Ok(())
}

fn digest_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl ReviewTarget {
    fn validate(&self) -> Result<(), CuratorJobRefusal> {
        match self {
            Self::StagedSubject {
                kernel_incarnation,
                candidate_id,
                payload_digest,
            } => {
                if !is_lower_hex(kernel_incarnation, 32) || !is_lower_hex(payload_digest, 64) {
                    return Err(CuratorJobRefusal::InvalidRequest);
                }
                check_identity(candidate_id)
            }
            Self::Memory {
                object_id,
                source_revision,
            } => {
                if *source_revision < 0 {
                    return Err(CuratorJobRefusal::InvalidRequest);
                }
                check_identity(object_id)
            }
        }
    }

    fn encode(&self) -> Result<String, CuratorJobRefusal> {
        let text = serde_json::to_string(self).map_err(|_| CuratorJobRefusal::InvalidRequest)?;
        if text.len() > MAX_TARGET_JSON_BYTES {
            return Err(CuratorJobRefusal::InvalidRequest);
        }
        Ok(text)
    }
}

impl CausalInputs {
    /// Sorted, deduplicated, and validated; identity derives from this form.
    fn normalized(mut self) -> Result<Self, CuratorJobRefusal> {
        self.target.validate()?;
        check_identity(&self.question_template)?;
        self.signals.sort();
        self.signals.dedup();
        self.required_evidence.sort();
        self.required_evidence.dedup();
        // One availability per evidence id; a producer that reports both is inconsistent, not causally distinct.
        if self
            .required_evidence
            .windows(2)
            .any(|pair| pair[0].evidence_id == pair[1].evidence_id)
        {
            return Err(CuratorJobRefusal::InvalidRequest);
        }
        if self.signals.len() > MAX_CAUSAL_SIGNALS
            || self.required_evidence.len() > MAX_REQUIRED_EVIDENCE
            || self.policy_versions.len() > MAX_CAUSAL_POLICY_VERSIONS
        {
            return Err(CuratorJobRefusal::InvalidRequest);
        }
        for signal in &self.signals {
            check_identity(signal)?;
        }
        for evidence in &self.required_evidence {
            check_identity(&evidence.evidence_id)?;
        }
        for (name, version) in &self.policy_versions {
            check_identity(name)?;
            check_identity(version)?;
        }
        Ok(self)
    }

    /// Digest of everything but the target.
    fn fingerprint(&self) -> Result<String, CuratorJobRefusal> {
        #[derive(Serialize)]
        struct Fingerprint<'a> {
            question_template: &'a str,
            signals: &'a [String],
            required_evidence: &'a [EvidenceAvailability],
            policy_versions: &'a BTreeMap<String, String>,
        }
        let bytes = serde_json::to_vec(&Fingerprint {
            question_template: &self.question_template,
            signals: &self.signals,
            required_evidence: &self.required_evidence,
            policy_versions: &self.policy_versions,
        })
        .map_err(|_| CuratorJobRefusal::InvalidRequest)?;
        Ok(digest_hex(&bytes))
    }

    /// The causal review identity: target digest joined with the fingerprint.
    pub fn causal_identity(&self) -> Result<String, CuratorJobRefusal> {
        let normalized = self.clone().normalized()?;
        let target = normalized.target.encode()?;
        let fingerprint = normalized.fingerprint()?;
        Ok(digest_hex(
            format!("{target}\u{1f}{fingerprint}").as_bytes(),
        ))
    }

    /// Every caller string, scanned raw at the transaction ceiling before any of it is serialized. The trigger scans stored columns too, but a keyed-JSON credential escaped inside `target_json`, `input_json`, or `page_json` is past the detector, and fingerprinted fields never reach a column at all.
    fn reject_secrets(&self) -> rusqlite::Result<()> {
        let strings = std::iter::once(self.target.identity())
            .chain(std::iter::once(&self.question_template))
            .chain(&self.signals)
            .chain(self.required_evidence.iter().map(|e| &e.evidence_id))
            .chain(self.policy_versions.iter().flat_map(|(n, v)| [n, v]));
        strings.map(String::as_str).try_for_each(reject_secret)
    }
}

fn reject_secret(text: &str) -> rusqlite::Result<()> {
    reject_transaction_secret_text(text)
        .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
}

impl ReviewTarget {
    /// The caller-chosen identity in a target; the incarnation, digest, and revision are validated hex or integers.
    fn identity(&self) -> &String {
        match self {
            Self::StagedSubject { candidate_id, .. } => candidate_id,
            Self::Memory { object_id, .. } => object_id,
        }
    }
}

impl CuratorJobInput {
    fn reject_secrets(&self) -> rusqlite::Result<()> {
        std::iter::once(self.subject.identity())
            .chain(std::iter::once(&self.question_template))
            .chain(&self.starting_references)
            .map(String::as_str)
            .try_for_each(reject_secret)
    }

    fn encode(&self) -> Result<String, CuratorJobRefusal> {
        self.subject.validate()?;
        check_identity(&self.question_template)?;
        if self.starting_references.len() > MAX_CURATOR_STARTING_REFERENCES {
            return Err(CuratorJobRefusal::InvalidRequest);
        }
        for reference in &self.starting_references {
            check_identity(reference)?;
        }
        let text = serde_json::to_string(self).map_err(|_| CuratorJobRefusal::InvalidRequest)?;
        if text.len() > MAX_CURATOR_JOB_INPUT_BYTES {
            return Err(CuratorJobRefusal::InvalidRequest);
        }
        Ok(text)
    }
}

impl ProducerBinding {
    fn validate(&self) -> Result<i64, CuratorJobRefusal> {
        if self.producer.is_empty() || self.producer.len() > 64 {
            return Err(CuratorJobRefusal::InvalidRequest);
        }
        check_identity(&self.firing_id)?;
        i64::try_from(self.ordinal).map_err(|_| CuratorJobRefusal::InvalidRequest)
    }
}

const JOB_COLUMNS: &str = "project, causal_identity, producer, firing_id, ordinal, target_json,
     input_fingerprint, state, input_json, outcome, queue_deadline_ms, created_at_ms, updated_at_ms,
     question_template";

/// Names a stored column whose value failed to decode; the value stays out of the error so no stored caller text reaches logs.
fn undecodable(column: usize, name: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(column, name.to_string(), rusqlite::types::Type::Text)
}

fn job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CuratorJob> {
    let target_json: String = row.get(5)?;
    let target = serde_json::from_str(&target_json).map_err(|_| undecodable(5, "target_json"))?;
    let state: String = row.get(7)?;
    let input_json: Option<String> = row.get(8)?;
    let outcome: Option<String> = row.get(9)?;
    let state = match (state.as_str(), input_json, outcome) {
        ("reserved", None, None) => CuratorJobState::Reserved,
        ("ready", Some(input), None) => CuratorJobState::Ready(
            serde_json::from_str(&input).map_err(|_| undecodable(8, "input_json"))?,
        ),
        ("terminal", _, Some(outcome)) => CuratorJobState::Terminal(
            CuratorJobOutcome::parse(&outcome).ok_or_else(|| undecodable(9, "outcome"))?,
        ),
        _ => return Err(undecodable(7, "state")),
    };
    Ok(CuratorJob {
        project: row.get(0)?,
        causal_identity: row.get(1)?,
        producer: ProducerBinding {
            producer: row.get(2)?,
            firing_id: row.get(3)?,
            ordinal: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
        },
        target,
        question_template: row.get(13)?,
        input_fingerprint: row.get(6)?,
        state,
        queue_deadline_ms: row.get(10)?,
        created_at_ms: row.get(11)?,
        updated_at_ms: row.get(12)?,
    })
}

pub(crate) fn load_curator_job(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
) -> rusqlite::Result<Option<CuratorJob>> {
    conn.query_row(
        &format!(
            "SELECT {JOB_COLUMNS} FROM curator_jobs WHERE project = ?1 AND causal_identity = ?2"
        ),
        params![project, causal_identity],
        job_from_row,
    )
    .optional()
}

/// Runs `project_sql` for one project and `host_sql` for the whole host, so neither text carries an optional project predicate that would defeat its index.
fn scalar(
    conn: &GuardedConn<'_>,
    project: Option<&str>,
    project_sql: &str,
    host_sql: &str,
) -> rusqlite::Result<i64> {
    match project {
        Some(project) => conn.query_row(project_sql, [project], |row| row.get(0)),
        None => conn.query_row(host_sql, [], |row| row.get(0)),
    }
}

/// Receipt charges of every row plus allowances of non-terminal rows and frozen pages.
pub(crate) fn metadata_bytes(
    conn: &GuardedConn<'_>,
    project: Option<&str>,
) -> rusqlite::Result<u64> {
    let jobs = scalar(
        conn,
        project,
        "SELECT COALESCE(SUM(receipt_charge_bytes + allowance_bytes), 0) FROM curator_jobs
         WHERE project = ?1",
        "SELECT COALESCE(SUM(receipt_charge_bytes + allowance_bytes), 0) FROM curator_jobs",
    )?;
    let pages = scalar(
        conn,
        project,
        "SELECT COALESCE(SUM(receipt_charge_bytes + allowance_bytes), 0)
         FROM curator_frozen_selections WHERE project = ?1",
        "SELECT COALESCE(SUM(receipt_charge_bytes + allowance_bytes), 0)
         FROM curator_frozen_selections",
    )?;
    Ok(u64::try_from(jobs)
        .unwrap_or(u64::MAX)
        .saturating_add(u64::try_from(pages).unwrap_or(u64::MAX)))
}

fn pending_jobs(conn: &GuardedConn<'_>, project: Option<&str>) -> rusqlite::Result<usize> {
    let count = scalar(
        conn,
        project,
        "SELECT COUNT(*) FROM curator_jobs
         WHERE project = ?1 AND state IN ('reserved', 'ready')",
        "SELECT COUNT(*) FROM curator_jobs WHERE state IN ('reserved', 'ready')",
    )?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

fn frozen_pages(conn: &GuardedConn<'_>, project: Option<&str>) -> rusqlite::Result<usize> {
    let count = scalar(
        conn,
        project,
        "SELECT COUNT(*) FROM curator_frozen_selections
         WHERE project = ?1 AND state = 'frozen'",
        "SELECT COUNT(*) FROM curator_frozen_selections WHERE state = 'frozen'",
    )?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// Refuses when adding `charge` bytes would exceed either metadata quota. `credit` is what the same transaction releases before it commits (a frozen page's allowance once its references become jobs), so headroom is judged against the committed state without writing anything first.
fn check_quota(
    conn: &GuardedConn<'_>,
    project: &str,
    charge: u64,
    credit: u64,
) -> rusqlite::Result<()> {
    let fits = |held: u64, bound: u64| held.saturating_sub(credit).saturating_add(charge) <= bound;
    if !fits(
        metadata_bytes(conn, Some(project))?,
        MAX_CURATOR_METADATA_BYTES_PER_PROJECT,
    ) || !fits(
        metadata_bytes(conn, None)?,
        MAX_CURATOR_METADATA_BYTES_PER_HOST,
    ) {
        return Err(refuse(CuratorJobRefusal::MetadataQuota));
    }
    Ok(())
}

/// Reserves one job for `inputs` inside the caller's fenced transaction, or returns the row the same causal identity already has. The queue deadline is `now_ms` plus [`CURATOR_QUEUE_LIFETIME_MS`] and never moves afterwards.
pub fn reserve_curator_job_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    producer: &ProducerBinding,
    inputs: &CausalInputs,
    now_ms: i64,
) -> rusqlite::Result<ReserveOutcome> {
    check_identity(project).map_err(refuse)?;
    producer.validate().map_err(refuse)?;
    let inputs = inputs.clone().normalized().map_err(refuse)?;
    let causal_identity = inputs.causal_identity().map_err(refuse)?;
    inputs.reject_secrets()?;
    if let Some(existing) = load_curator_job(conn, project, &causal_identity)? {
        return Ok(ReserveOutcome::Existing(existing));
    }
    check_reservation_headroom(conn, project, 1, 0)?;
    insert_reservation_in_tx(conn, project, producer, &inputs, &causal_identity, now_ms)
        .map(ReserveOutcome::Reserved)
}

/// Refuses unless `count` new reservations fit under both pending-job caps and both metadata quotas, after `credit` bytes the same transaction releases. Capacity is judged before quota, so a caller sees the refusal that clears first.
fn check_reservation_headroom(
    conn: &GuardedConn<'_>,
    project: &str,
    count: usize,
    credit: u64,
) -> rusqlite::Result<()> {
    if pending_jobs(conn, Some(project))?.saturating_add(count)
        > MAX_PENDING_CURATOR_JOBS_PER_PROJECT
    {
        return Err(refuse(CuratorJobRefusal::ProjectCapacity));
    }
    if pending_jobs(conn, None)?.saturating_add(count) > MAX_PENDING_CURATOR_JOBS_PER_HOST {
        return Err(refuse(CuratorJobRefusal::HostCapacity));
    }
    check_quota(
        conn,
        project,
        (CURATOR_RECEIPT_CHARGE_BYTES + CURATOR_JOB_ALLOWANCE_BYTES)
            .saturating_mul(u64::try_from(count).unwrap_or(u64::MAX)),
        credit,
    )
}

/// Writes one `reserved` row for already-normalized `inputs`; the caller has checked headroom for it.
fn insert_reservation_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    producer: &ProducerBinding,
    inputs: &CausalInputs,
    causal_identity: &str,
    now_ms: i64,
) -> rusqlite::Result<CuratorJob> {
    let ordinal = producer.validate().map_err(refuse)?;
    let deadline = now_ms
        .checked_add(CURATOR_QUEUE_LIFETIME_MS)
        .ok_or_else(|| refuse(CuratorJobRefusal::InvalidRequest))?;
    conn.execute(
        "INSERT INTO curator_jobs (
             project, causal_identity, producer, firing_id, ordinal, target_json, question_template,
             input_fingerprint, state, queue_deadline_ms, allowance_bytes, receipt_charge_bytes,
             created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'reserved', ?9, ?10, ?11, ?12, ?12)",
        params![
            project,
            causal_identity,
            producer.producer,
            producer.firing_id,
            ordinal,
            inputs.target.encode().map_err(refuse)?,
            inputs.question_template,
            inputs.fingerprint().map_err(refuse)?,
            deadline,
            i64::try_from(CURATOR_JOB_ALLOWANCE_BYTES).unwrap_or(i64::MAX),
            i64::try_from(CURATOR_RECEIPT_CHARGE_BYTES).unwrap_or(i64::MAX),
            now_ms,
        ],
    )?;
    load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))
}

/// Moves a `Reserved` row to `Ready` with reference-only input inside the caller's fenced transaction. Activation consumes no second capacity slot and is refused for a row that is not reserved, belongs to another producer firing, has passed its queue deadline, or whose input names a subject or question template other than the reserved ones.
pub fn activate_curator_job_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    producer: &ProducerBinding,
    input: &CuratorJobInput,
    now_ms: i64,
) -> rusqlite::Result<CuratorJob> {
    producer.validate().map_err(refuse)?;
    input.reject_secrets()?;
    let input_json = input.encode().map_err(refuse)?;
    let job = load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    match job.state {
        CuratorJobState::Reserved => {}
        CuratorJobState::Ready(_) => return Err(refuse(CuratorJobRefusal::NotReserved)),
        CuratorJobState::Terminal(_) => return Err(refuse(CuratorJobRefusal::Terminal)),
    }
    if job.producer != *producer {
        return Err(refuse(CuratorJobRefusal::ProducerMismatch));
    }
    if job.queue_deadline_ms <= now_ms {
        return Err(refuse(CuratorJobRefusal::Expired));
    }
    if input.subject != job.target || input.question_template != job.question_template {
        return Err(refuse(CuratorJobRefusal::InvalidRequest));
    }
    conn.execute(
        "UPDATE curator_jobs SET state = 'ready', input_json = ?3, updated_at_ms = ?4
         WHERE project = ?1 AND causal_identity = ?2 AND state = 'reserved'",
        params![project, causal_identity, input_json, now_ms],
    )?;
    load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))
}

/// Rebinds a `Reserved` row to another firing of the same producer. Preserves its deadline, target, and allowance. Refuses non-reserved, foreign-producer, or expired rows.
pub fn rebind_reserved_curator_job_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    producer: &ProducerBinding,
    now_ms: i64,
) -> rusqlite::Result<CuratorJob> {
    let ordinal = producer.validate().map_err(refuse)?;
    let job = load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    match job.state {
        CuratorJobState::Reserved => {}
        CuratorJobState::Ready(_) => return Err(refuse(CuratorJobRefusal::NotReserved)),
        CuratorJobState::Terminal(_) => return Err(refuse(CuratorJobRefusal::Terminal)),
    }
    if job.producer.producer != producer.producer {
        return Err(refuse(CuratorJobRefusal::ProducerMismatch));
    }
    if job.queue_deadline_ms <= now_ms {
        return Err(refuse(CuratorJobRefusal::Expired));
    }
    conn.execute(
        "UPDATE curator_jobs SET firing_id = ?3, ordinal = ?4, updated_at_ms = ?5
         WHERE project = ?1 AND causal_identity = ?2 AND state = 'reserved'",
        params![
            project,
            causal_identity,
            producer.firing_id,
            ordinal,
            now_ms
        ],
    )?;
    load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))
}

/// Records one terminal outcome for a non-terminal row, drops its input, and releases its allowance; the compact receipt and its charge stay. A terminal row is never reopened, and a finish at or after the queue deadline is refused as expired so the permanent outcome does not depend on whether the sweep ran first.
pub fn finish_curator_job_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    outcome: CuratorJobOutcome,
    now_ms: i64,
) -> rusqlite::Result<CuratorJob> {
    let changed = conn.execute(
        "UPDATE curator_jobs
            SET state = 'terminal', outcome = ?3, allowance_bytes = 0, input_json = NULL,
                updated_at_ms = ?4
          WHERE project = ?1 AND causal_identity = ?2 AND state <> 'terminal'
            AND queue_deadline_ms > ?4",
        params![project, causal_identity, outcome.as_str(), now_ms],
    )?;
    let job = load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    if changed == 0 {
        return Err(refuse(match job.state {
            CuratorJobState::Terminal(_) => CuratorJobRefusal::Terminal,
            _ => CuratorJobRefusal::Expired,
        }));
    }
    Ok(job)
}

/// Closes a `Reserved` row whose queue deadline has passed with the same `expired` outcome the sweep would record, so an activation that finds its reservation expired can close it eagerly without the outcome depending on whether the sweep ran first. A row still inside its deadline, already terminal, or past reservation is refused.
pub fn expire_reserved_curator_job_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    causal_identity: &str,
    now_ms: i64,
) -> rusqlite::Result<CuratorJob> {
    let changed = conn.execute(
        "UPDATE curator_jobs
            SET state = 'terminal', outcome = 'expired', allowance_bytes = 0, input_json = NULL,
                updated_at_ms = ?3
          WHERE project = ?1 AND causal_identity = ?2 AND state = 'reserved'
            AND queue_deadline_ms <= ?3",
        params![project, causal_identity, now_ms],
    )?;
    let job = load_curator_job(conn, project, causal_identity)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    if changed == 0 {
        return Err(refuse(match job.state {
            CuratorJobState::Terminal(_) => CuratorJobRefusal::Terminal,
            CuratorJobState::Ready(_) => CuratorJobRefusal::NotReserved,
            CuratorJobState::Reserved => CuratorJobRefusal::InvalidRequest,
        }));
    }
    Ok(job)
}

/// Freezes one selection page for a slot attempt. One frozen page per project and 32 per host; the page keeps its references and cursor until [`complete_frozen_selection_in_tx`] moves it out of `frozen`. An attempt identity freezes exactly one page: resubmitting that page replays the row, and a different page under the same identity is refused.
pub fn freeze_selection_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    slot_id: &str,
    selection_attempt: &str,
    page: &FrozenSelectionPage,
    now_ms: i64,
) -> rusqlite::Result<FrozenSelection> {
    check_identity(project).map_err(refuse)?;
    check_identity(slot_id).map_err(refuse)?;
    check_identity(selection_attempt).map_err(refuse)?;
    if page.references.is_empty()
        || page.references.len() > MAX_SELECTION_REFERENCES
        || page
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() > 512)
    {
        return Err(refuse(CuratorJobRefusal::InvalidRequest));
    }
    let mut normalized = Vec::with_capacity(page.references.len());
    for inputs in &page.references {
        let inputs = inputs.clone().normalized().map_err(refuse)?;
        inputs.reject_secrets()?;
        normalized.push(inputs);
    }
    if let Some(cursor) = &page.next_cursor {
        reject_secret(cursor)?;
    }
    let page = FrozenSelectionPage {
        references: normalized,
        next_cursor: page.next_cursor.clone(),
    };
    let page_json =
        serde_json::to_string(&page).map_err(|_| refuse(CuratorJobRefusal::InvalidRequest))?;
    if page_json.len() > MAX_FROZEN_PAGE_BYTES {
        return Err(refuse(CuratorJobRefusal::PageTooLarge));
    }
    if let Some(existing) = load_selection(conn, project, slot_id, selection_attempt)? {
        return match &existing.page {
            Some(frozen) if existing.state == FrozenSelectionState::Frozen => {
                if *frozen == page {
                    Ok(existing)
                } else {
                    Err(refuse(CuratorJobRefusal::InvalidRequest))
                }
            }
            _ => Err(refuse(CuratorJobRefusal::Terminal)),
        };
    }
    if frozen_pages(conn, Some(project))? >= MAX_FROZEN_SELECTIONS_PER_PROJECT {
        return Err(refuse(CuratorJobRefusal::ProjectSelectionCapacity));
    }
    if frozen_pages(conn, None)? >= MAX_FROZEN_SELECTIONS_PER_HOST {
        return Err(refuse(CuratorJobRefusal::HostSelectionCapacity));
    }
    check_quota(
        conn,
        project,
        FROZEN_PAGE_RECEIPT_CHARGE_BYTES + FROZEN_SELECTION_ALLOWANCE_BYTES,
        0,
    )?;
    let deadline = now_ms
        .checked_add(CURATOR_QUEUE_LIFETIME_MS)
        .ok_or_else(|| refuse(CuratorJobRefusal::InvalidRequest))?;
    conn.execute(
        "INSERT INTO curator_frozen_selections (
             project, slot_id, selection_attempt, page_json, reference_count, next_cursor, state,
             selection_deadline_ms, allowance_bytes, receipt_charge_bytes, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'frozen', ?7, ?8, ?9, ?10, ?10)",
        params![
            project,
            slot_id,
            selection_attempt,
            page_json,
            i64::try_from(page.references.len()).unwrap_or(0),
            page.next_cursor,
            deadline,
            i64::try_from(FROZEN_SELECTION_ALLOWANCE_BYTES).unwrap_or(i64::MAX),
            i64::try_from(FROZEN_PAGE_RECEIPT_CHARGE_BYTES).unwrap_or(i64::MAX),
            now_ms,
        ],
    )?;
    load_selection(conn, project, slot_id, selection_attempt)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))
}

/// Moves a frozen page to a terminal state, dropping its references and cursor and releasing its allowance; the receipt charge stays. An `Enqueued` result still carries the page, so the caller advances its slot to `next_cursor` in the same transaction; `Expired` and `FailedSlot` return a compact receipt and leave the cursor where it was. Any completion at or after the selection deadline is refused as expired and left for the sweep, so the receipt does not depend on transaction ordering. Capacity deferral is not a state: the page stays `frozen` in its slot for a later attempt.
pub fn complete_frozen_selection_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    slot_id: &str,
    selection_attempt: &str,
    state: FrozenSelectionState,
    now_ms: i64,
) -> rusqlite::Result<FrozenSelection> {
    if state == FrozenSelectionState::Frozen {
        return Err(refuse(CuratorJobRefusal::InvalidRequest));
    }
    let existing = load_selection(conn, project, slot_id, selection_attempt)?
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    if existing.state != FrozenSelectionState::Frozen {
        return Err(refuse(CuratorJobRefusal::Terminal));
    }
    if existing.selection_deadline_ms <= now_ms {
        return Err(refuse(CuratorJobRefusal::Expired));
    }
    retire_frozen_page_in_tx(conn, project, slot_id, selection_attempt, state, now_ms)?;
    Ok(FrozenSelection {
        state,
        page: existing
            .page
            .filter(|_| state == FrozenSelectionState::Enqueued),
        ..existing
    })
}

/// The terminal write for one frozen row: `state`, references and cursor dropped, allowance released. The scan audit of the references is retired with them unless they became jobs (`Enqueued`), whose rows keep the same identities for the store incarnation and so keep their audit. Callers decide whether the deadline permits it.
fn retire_frozen_page_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    slot_id: &str,
    selection_attempt: &str,
    state: FrozenSelectionState,
    now_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE curator_frozen_selections
            SET state = ?4, page_json = NULL, next_cursor = NULL, allowance_bytes = 0,
                updated_at_ms = ?5
          WHERE project = ?1 AND slot_id = ?2 AND selection_attempt = ?3 AND state = 'frozen'",
        params![project, slot_id, selection_attempt, state.as_str(), now_ms],
    )?;
    if state == FrozenSelectionState::Enqueued {
        return Ok(());
    }
    release_frozen_page_scans(conn, project, slot_id, selection_attempt)
}

/// The scan owner of one page's reference and cursor text; retired when the page goes terminal and that text leaves the row.
fn frozen_page_owner_key(slot_id: &str, selection_attempt: &str) -> String {
    active_scan_owner_key(&["curator", "freeze", slot_id, selection_attempt])
}

fn release_frozen_page_scans(
    conn: &GuardedConn<'_>,
    project: &str,
    slot_id: &str,
    selection_attempt: &str,
) -> rusqlite::Result<()> {
    retire_active_scan_domain_owner(
        conn,
        "project",
        project,
        DurableWriteFamily::CuratorJobs.owner_kind(),
        &frozen_page_owner_key(slot_id, selection_attempt),
    )
}

/// How an enqueue attempt ended inside its transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// Every reference has a ready job; `jobs` were newly reserved and activated, `replayed` already existed at the same causal inputs.
    Enqueued {
        jobs: usize,
        replayed: usize,
        next_cursor: Option<String>,
    },
    /// Capacity or quota refused a reference; nothing was written and the page stays frozen.
    Deferred(CuratorJobRefusal),
    /// The page passed its selection deadline before it could be enqueued; the slot is recorded as failed.
    Expired,
    /// The slot's completion had already been recorded under this completion id; the earlier outcome stands and nothing was written.
    Replayed,
}

/// Records where `slot_id`'s next selection resumes. A `None` cursor means the pass reached the end.
pub fn advance_selection_cursor_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    slot_id: &str,
    cursor: Option<&str>,
    now_ms: i64,
) -> rusqlite::Result<()> {
    check_identity(project).map_err(refuse)?;
    check_identity(slot_id).map_err(refuse)?;
    if cursor.is_some_and(|cursor| cursor.len() > 512) {
        return Err(refuse(CuratorJobRefusal::InvalidRequest));
    }
    conn.execute(
        "INSERT INTO curator_selection_cursors (project, slot_id, cursor, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project, slot_id) DO UPDATE SET cursor = excluded.cursor, updated_at_ms = excluded.updated_at_ms",
        params![project, slot_id, cursor, now_ms],
    )?;
    Ok(())
}

/// The transaction-local enqueue: reserve and activate every reference the page does not already have a row for, then move the page to `enqueued`. The page must be `frozen`; a capacity or quota refusal for the page's new rows surfaces as `Deferred` before any row is written, so the caller rolls the transaction back rather than completing the slot.
pub fn enqueue_frozen_selection_in_tx(
    conn: &GuardedConn<'_>,
    project: &str,
    selection: &FrozenSelection,
    producer: &ProducerBinding,
    now_ms: i64,
) -> rusqlite::Result<EnqueueOutcome> {
    let existing = load_selection(
        conn,
        project,
        &selection.slot_id,
        &selection.selection_attempt,
    )?
    .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    if existing.state != FrozenSelectionState::Frozen {
        return Err(refuse(CuratorJobRefusal::Terminal));
    }
    // A page at or past its deadline is retired here the way the sweep retires it, since the pre-deadline completion refuses it: the slot records the failure instead of retrying an enqueue that can never succeed.
    if existing.selection_deadline_ms <= now_ms {
        retire_frozen_page_in_tx(
            conn,
            project,
            &selection.slot_id,
            &selection.selection_attempt,
            FrozenSelectionState::FailedSlot,
            now_ms,
        )?;
        return Ok(EnqueueOutcome::Expired);
    }
    // A frozen row carries its page; the row's own check enforces it.
    let page = existing
        .page
        .as_ref()
        .ok_or_else(|| refuse(CuratorJobRefusal::Missing))?;
    // Headroom is judged once for the whole page against the committed state, crediting the allowance the page releases when it leaves `frozen` later in this transaction; nothing is written before the judgement, so a `Deferred` outcome leaves the row untouched whatever the caller does with its transaction. A page either fits entirely or is deferred entirely.
    producer.validate().map_err(refuse)?;
    let mut fresh = Vec::with_capacity(page.references.len());
    let mut replayed = 0usize;
    for inputs in &page.references {
        let inputs = inputs.clone().normalized().map_err(refuse)?;
        let causal_identity = inputs.causal_identity().map_err(refuse)?;
        if load_curator_job(conn, project, &causal_identity)?.is_some()
            || fresh
                .iter()
                .any(|(_, fresh_id)| *fresh_id == causal_identity)
        {
            replayed += 1;
        } else {
            fresh.push((inputs, causal_identity));
        }
    }
    if !fresh.is_empty()
        && let Err(error) =
            check_reservation_headroom(conn, project, fresh.len(), FROZEN_SELECTION_ALLOWANCE_BYTES)
    {
        return match refusal_of(&error) {
            Some(
                reason @ (CuratorJobRefusal::ProjectCapacity
                | CuratorJobRefusal::HostCapacity
                | CuratorJobRefusal::MetadataQuota),
            ) => Ok(EnqueueOutcome::Deferred(reason)),
            _ => Err(error),
        };
    }
    let jobs = fresh.len();
    for (inputs, causal_identity) in &fresh {
        insert_reservation_in_tx(conn, project, producer, inputs, causal_identity, now_ms)?;
        activate_curator_job_in_tx(
            conn,
            project,
            causal_identity,
            producer,
            &CuratorJobInput {
                subject: inputs.target.clone(),
                starting_references: Vec::new(),
                question_template: inputs.question_template.clone(),
            },
            now_ms,
        )?;
    }
    complete_frozen_selection_in_tx(
        conn,
        project,
        &selection.slot_id,
        &selection.selection_attempt,
        FrozenSelectionState::Enqueued,
        now_ms,
    )?;
    advance_selection_cursor_in_tx(
        conn,
        project,
        &selection.slot_id,
        page.next_cursor.as_deref(),
        now_ms,
    )?;
    Ok(EnqueueOutcome::Enqueued {
        jobs,
        replayed,
        next_cursor: page.next_cursor.clone(),
    })
}

fn load_selection(
    conn: &GuardedConn<'_>,
    project: &str,
    slot_id: &str,
    selection_attempt: &str,
) -> rusqlite::Result<Option<FrozenSelection>> {
    conn.query_row(
        "SELECT page_json, state, selection_deadline_ms, created_at_ms FROM curator_frozen_selections
         WHERE project = ?1 AND slot_id = ?2 AND selection_attempt = ?3",
        params![project, slot_id, selection_attempt],
        |row| {
            let page_json: Option<String> = row.get(0)?;
            let state: String = row.get(1)?;
            let page = page_json
                .map(|page_json| serde_json::from_str(&page_json))
                .transpose()
                .map_err(|_| undecodable(0, "page_json"))?;
            Ok(FrozenSelection {
                project: project.to_string(),
                slot_id: slot_id.to_string(),
                selection_attempt: selection_attempt.to_string(),
                page,
                state: FrozenSelectionState::parse(&state)
                    .ok_or_else(|| undecodable(1, "state"))?,
                selection_deadline_ms: row.get(2)?,
                created_at_ms: row.get(3)?,
            })
        },
    )
    .optional()
}

/// Every free string a causal input carries is an identity: a detected secret refuses the write instead of being redacted into a different identity.
fn scan_causal_identities(
    write: &mut PreparedWrite,
    inputs: &CausalInputs,
) -> Result<(), MemoryStoreError> {
    let CausalInputs {
        target,
        question_template,
        signals,
        required_evidence,
        policy_versions,
    } = inputs;
    scan_target(write, target)?;
    write.identity("question_template", question_template)?;
    for signal in signals {
        write.identity("signal", signal)?;
    }
    for EvidenceAvailability { evidence_id, .. } in required_evidence {
        write.identity("evidence_id", evidence_id)?;
    }
    for (name, version) in policy_versions {
        write.identity("policy_name", name)?;
        write.identity("policy_version", version)?;
    }
    Ok(())
}

fn scan_target(write: &mut PreparedWrite, target: &ReviewTarget) -> Result<(), MemoryStoreError> {
    match target {
        ReviewTarget::StagedSubject {
            kernel_incarnation: _,
            candidate_id,
            payload_digest: _,
        } => write.identity("candidate_id", candidate_id)?,
        ReviewTarget::Memory {
            object_id,
            source_revision: _,
        } => write.identity("object_id", object_id)?,
    };
    Ok(())
}

/// The prepared write every Curator family operation runs under: the project is bounded and scanned as an existing identity, and the scan audit is owned by `owner_parts`.
pub(crate) fn curator_write(
    project: &str,
    owner_parts: &[&str],
) -> Result<PreparedWrite, MemoryStoreError> {
    check_project(project)?;
    let mut write = PreparedWrite::new(DurableWriteFamily::CuratorJobs);
    write.domain_owner("project", project, active_scan_owner_key(owner_parts));
    write.existing_identity("project", project)?;
    Ok(write)
}

/// The store's own durable incarnation, read inside the caller's transaction.
pub(crate) fn store_incarnation_in_tx(conn: &GuardedConn<'_>) -> rusqlite::Result<String> {
    conn.query_row(
        "SELECT database_incarnation_id FROM curator_store_identity WHERE id = 0",
        [],
        |row| row.get(0),
    )
}

impl MemoryStore {
    /// The store's own durable incarnation, written once at genesis; reopen keeps it and a replaced file has another.
    pub fn curator_store_incarnation(&self) -> Result<String, MemoryStoreError> {
        self.inner
            .with_conn(store_incarnation_in_tx)
            .map_err(Into::into)
    }

    /// Writes the incarnation row when the file has none; `open` calls this once.
    pub(crate) fn ensure_curator_store_identity(
        &self,
        now_ms: i64,
    ) -> Result<(), MemoryStoreError> {
        let present: bool = self.inner.with_conn(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM curator_store_identity WHERE id = 0)",
                [],
                |row| row.get(0),
            )
        })?;
        if present {
            return Ok(());
        }
        self.inner.with_conn_fenced(|conn| {
            conn.execute(
                "INSERT INTO curator_store_identity (id, database_incarnation_id, created_at_ms)
                 SELECT 0, lower(hex(randomblob(16))), ?1
                 WHERE NOT EXISTS (SELECT 1 FROM curator_store_identity WHERE id = 0)",
                [now_ms],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// One fenced write: `prepare` records the identities the write carries so a detected secret refuses it, then `body` runs the transaction-local primitive.
    fn curator_transaction<T>(
        &self,
        project: &str,
        operation: &str,
        prepare: impl FnOnce(&mut PreparedWrite) -> Result<(), MemoryStoreError>,
        body: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<WriteDisposition<T>>,
    ) -> Result<T, CuratorJobError> {
        let mut write = curator_write(project, &["curator", operation])?;
        prepare(&mut write)?;
        let refusal = std::cell::Cell::new(None);
        let result = write.execute(&self.inner, |coordinated| {
            body(coordinated.tx()).inspect_err(|error| refusal.set(refusal_of(error)))
        });
        match (result, refusal.get()) {
            (Err(_), Some(refusal)) => Err(CuratorJobError::Refused(refusal)),
            (Err(error), None) => Err(CuratorJobError::Store(error)),
            (Ok(value), _) => Ok(value),
        }
    }

    pub fn reserve_curator_job(
        &self,
        project: &str,
        producer: &ProducerBinding,
        inputs: &CausalInputs,
        now_ms: i64,
    ) -> Result<ReserveOutcome, CuratorJobError> {
        self.curator_transaction(
            project,
            "reserve",
            |write| {
                write.identity("producer", &producer.producer)?;
                write.identity("firing_id", &producer.firing_id)?;
                scan_causal_identities(write, inputs)
            },
            |conn| {
                let outcome = reserve_curator_job_in_tx(conn, project, producer, inputs, now_ms)?;
                Ok(match outcome {
                    ReserveOutcome::Existing(_) => WriteDisposition::Replay(outcome),
                    ReserveOutcome::Reserved(_) => WriteDisposition::Applied(outcome),
                })
            },
        )
    }

    pub fn activate_curator_job(
        &self,
        project: &str,
        causal_identity: &str,
        producer: &ProducerBinding,
        input: &CuratorJobInput,
        now_ms: i64,
    ) -> Result<CuratorJob, CuratorJobError> {
        self.curator_transaction(
            project,
            "activate",
            |write| {
                let CuratorJobInput {
                    subject,
                    starting_references,
                    question_template,
                } = input;
                scan_target(write, subject)?;
                write.identity("question_template", question_template)?;
                for reference in starting_references {
                    write.identity("starting_reference", reference)?;
                }
                Ok(())
            },
            |conn| {
                activate_curator_job_in_tx(conn, project, causal_identity, producer, input, now_ms)
                    .map(WriteDisposition::Applied)
            },
        )
    }

    pub fn rebind_reserved_curator_job(
        &self,
        project: &str,
        causal_identity: &str,
        producer: &ProducerBinding,
        now_ms: i64,
    ) -> Result<CuratorJob, CuratorJobError> {
        self.curator_transaction(
            project,
            "rebind",
            |write| {
                write.identity("producer", &producer.producer)?;
                write.identity("firing_id", &producer.firing_id)?;
                Ok(())
            },
            |conn| {
                rebind_reserved_curator_job_in_tx(conn, project, causal_identity, producer, now_ms)
                    .map(WriteDisposition::Applied)
            },
        )
    }

    /// Closes a job from outside a run. `Completed` and `Abstained` are execution outcomes that only a receipt completion records, with the attempt and selection evidence behind them; they are refused here.
    pub fn finish_curator_job(
        &self,
        project: &str,
        causal_identity: &str,
        outcome: CuratorJobOutcome,
        now_ms: i64,
    ) -> Result<CuratorJob, CuratorJobError> {
        if matches!(
            outcome,
            CuratorJobOutcome::Completed | CuratorJobOutcome::Abstained
        ) {
            return Err(CuratorJobError::Refused(CuratorJobRefusal::InvalidRequest));
        }
        self.curator_transaction(
            project,
            "finish",
            |_| Ok(()),
            |conn| {
                let job =
                    finish_curator_job_in_tx(conn, project, causal_identity, outcome, now_ms)?;
                // A claim on a finished job can do nothing but renew; fence it here rather than leave it to the sweep.
                let job_id: i64 = conn.query_row(
                    "SELECT job_id FROM curator_jobs WHERE project = ?1 AND causal_identity = ?2",
                    params![project, causal_identity],
                    |row| row.get(0),
                )?;
                crate::task_lease::fence_task_claims_tx(
                    conn,
                    &crate::curator_ledger::CURATOR_REVIEW_TASK,
                    project,
                    job_id,
                    "stale",
                    now_ms,
                )?;
                Ok(WriteDisposition::Applied(job))
            },
        )
    }

    /// Expires reserved or ready jobs and frozen selections at their deadlines. An in-progress receipt inside its run deadline shields its job; at the run deadline the receipt closes as `cancelled` when a cancellation was recorded, `unknown` when it has an unterminated attempt, otherwise `expired`, and its job goes terminal in the same sweep with the outcome a completion would have mapped. Job expiry clears `allowance_bytes` and `input_json` without deleting receipts, and the live claim of any terminal job is fenced `expired`, so a worker that keeps renewing cannot hold a ledger slot for a job that no longer exists.
    pub fn expire_curator_work(&self, now_ms: i64) -> Result<(usize, usize), CuratorJobError> {
        // The sweep carries no caller text, so it records no scan and needs no owner scope.
        let write = PreparedWrite::new(DurableWriteFamily::CuratorJobs);
        write
            .execute(&self.inner, |coordinated| {
                // Nothing can publish once a receipt's run deadline has passed, so an in-progress receipt closes there whatever its job's state, and the job follows in the same transaction rather than holding a pending slot until its queue deadline.
                coordinated.tx().execute(
                    "UPDATE curator_receipts
                        SET state = 'complete', updated_at_ms = ?1,
                            terminal_kind = CASE
                              WHEN cancelled_at_ms IS NOT NULL THEN 'cancelled'
                              WHEN EXISTS(
                                SELECT 1 FROM curator_attempts a
                                 WHERE a.project = curator_receipts.project
                                   AND a.causal_identity = curator_receipts.causal_identity
                                   AND a.terminal_kind IS NULL)
                              THEN 'unknown' ELSE 'expired' END
                      WHERE state = 'in_progress' AND run_deadline_ms <= ?1",
                    [now_ms],
                )?;
                // A job expires at its queue deadline, or as soon as its receipt is closed; an in-progress receipt inside its run deadline still shields the job.
                let jobs = coordinated.tx().execute(
                    "UPDATE curator_jobs
                        SET state = 'terminal', allowance_bytes = 0, input_json = NULL, updated_at_ms = ?1,
                            outcome = COALESCE((
                                SELECT CASE r.terminal_kind
                                         WHEN 'unknown' THEN 'unknown'
                                         WHEN 'cancelled' THEN 'failed'
                                         ELSE 'expired' END
                                  FROM curator_receipts r
                                 WHERE r.project = curator_jobs.project
                                   AND r.causal_identity = curator_jobs.causal_identity), 'expired')
                      WHERE state IN ('reserved', 'ready')
                        AND (queue_deadline_ms <= ?1
                             OR EXISTS(SELECT 1 FROM curator_receipts r
                                        WHERE r.project = curator_jobs.project
                                          AND r.causal_identity = curator_jobs.causal_identity
                                          AND r.state = 'complete'))
                        AND NOT EXISTS(SELECT 1 FROM curator_receipts r
                                        WHERE r.project = curator_jobs.project
                                          AND r.causal_identity = curator_jobs.causal_identity
                                          AND r.state = 'in_progress')",
                    [now_ms],
                )?;
                let expiring_pages: Vec<(String, String, String)> = coordinated
                    .tx()
                    .prepare(
                        "SELECT project, slot_id, selection_attempt FROM curator_frozen_selections
                          WHERE state = 'frozen' AND selection_deadline_ms <= ?1",
                    )?
                    .query_map([now_ms], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                let pages = coordinated.tx().execute(
                    "UPDATE curator_frozen_selections
                        SET state = 'expired', page_json = NULL, next_cursor = NULL,
                            allowance_bytes = 0, updated_at_ms = ?1
                      WHERE state = 'frozen' AND selection_deadline_ms <= ?1",
                    [now_ms],
                )?;
                for (project, slot_id, selection_attempt) in &expiring_pages {
                    release_frozen_page_scans(coordinated.tx(), project, slot_id, selection_attempt)?;
                }
                // Renewal checks the claim, not the job, so a claim on a terminal job would otherwise live as long as its worker heartbeats.
                coordinated.tx().execute(
                    "UPDATE note_eval_claims
                        SET terminal_kind = 'expired', terminal_response = ?2, terminal_at_ms = ?1
                      WHERE task_kind = ?3 AND terminal_kind IS NULL
                        AND EXISTS(SELECT 1 FROM curator_jobs j
                                    WHERE j.project = note_eval_claims.project
                                      AND j.job_id = note_eval_claims.note_id
                                      AND j.state = 'terminal')",
                    params![
                        now_ms,
                        crate::task_lease::kind_response("expired"),
                        crate::curator_ledger::CURATOR_REVIEW_TASK.task_kind
                    ],
                )?;
                Ok(if jobs + pages == 0 {
                    WriteDisposition::Replay((0, 0))
                } else {
                    WriteDisposition::Applied((jobs, pages))
                })
            })
            .map_err(CuratorJobError::Store)
    }

    pub fn freeze_selection(
        &self,
        project: &str,
        slot_id: &str,
        selection_attempt: &str,
        page: &FrozenSelectionPage,
        now_ms: i64,
    ) -> Result<FrozenSelection, CuratorJobError> {
        self.curator_transaction(
            project,
            "freeze",
            |write| {
                write.identity("slot_id", slot_id)?;
                write.identity("selection_attempt", selection_attempt)?;
                // The references and cursor leave the row when the page goes terminal; their scans are owned by this page alone so they can be retired with them, while the slot and attempt identities the compact row keeps stay under the shared freeze owner.
                let first_reference_scan = write.scans.len();
                if let Some(cursor) = &page.next_cursor {
                    write.identity("next_cursor", cursor)?;
                }
                page.references
                    .iter()
                    .try_for_each(|inputs| scan_causal_identities(write, inputs))?;
                write.reassign_scans_in(
                    first_reference_scan..write.scans.len(),
                    "project",
                    project,
                    frozen_page_owner_key(slot_id, selection_attempt),
                );
                Ok(())
            },
            |conn| {
                // An identical page under an existing attempt replays the row; only a new row records its scan audit.
                let existed = load_selection(conn, project, slot_id, selection_attempt)?.is_some();
                let selection = freeze_selection_in_tx(
                    conn,
                    project,
                    slot_id,
                    selection_attempt,
                    page,
                    now_ms,
                )?;
                Ok(if existed {
                    WriteDisposition::Replay(selection)
                } else {
                    WriteDisposition::Applied(selection)
                })
            },
        )
    }

    /// Standalone terminal transition for `Expired` and `FailedSlot`, which move no cursor. `Enqueued` is refused here: the slot's cursor advance must commit with the transition, which only [`complete_frozen_selection_in_tx`] inside the caller's transaction can do.
    pub fn complete_frozen_selection(
        &self,
        project: &str,
        slot_id: &str,
        selection_attempt: &str,
        state: FrozenSelectionState,
        now_ms: i64,
    ) -> Result<FrozenSelection, CuratorJobError> {
        if state == FrozenSelectionState::Enqueued {
            return Err(CuratorJobRefusal::InvalidRequest.into());
        }
        self.curator_transaction(
            project,
            "complete-selection",
            |_| Ok(()),
            |conn| {
                complete_frozen_selection_in_tx(
                    conn,
                    project,
                    slot_id,
                    selection_attempt,
                    state,
                    now_ms,
                )
                .map(WriteDisposition::Applied)
            },
        )
    }

    /// Enqueues a frozen page and completes the scheduler slot that froze it in one fenced transaction: every reference is reserved and activated as a review job (identical causal inputs replay their existing row), the page moves to `enqueued`, the slot's continuation advances to the page's cursor, and the slot's lease completes. A pending-capacity or quota refusal on any reference rolls the whole transaction back: the page stays `frozen`, the lease stays claimed, and the cursor does not move, so a later attempt of the same slot serves the same page. An expired page completes the slot as `failed_slot` without advancing anything.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_frozen_selection(
        &self,
        project: &str,
        claim_id: &str,
        completion_id: &str,
        scheduler_instance: &str,
        slot: i64,
        selection: &FrozenSelection,
        producer: &ProducerBinding,
        now_ms: i64,
    ) -> Result<EnqueueOutcome, CuratorJobError> {
        let applied = std::cell::Cell::new(None);
        let outcome = self.complete_task_lease(
            &crate::MEMORY_CLASSIFIER_TASK,
            project,
            claim_id,
            completion_id,
            scheduler_instance,
            slot,
            now_ms,
            |coordinated, _claim| {
                let conn = coordinated.tx();
                let enqueue =
                    enqueue_frozen_selection_in_tx(conn, project, selection, producer, now_ms)?;
                let code = match &enqueue {
                    EnqueueOutcome::Enqueued { .. } => "curator_selection_enqueued",
                    EnqueueOutcome::Expired => "curator_selection_expired",
                    // The refusal ends the transaction; nothing below it is written and no terminal is recorded.
                    EnqueueOutcome::Deferred(reason) => {
                        let reason = *reason;
                        applied.set(Some(EnqueueOutcome::Deferred(reason)));
                        return Err(refuse(reason));
                    }
                    EnqueueOutcome::Replayed => unreachable!("the transaction never replays"),
                };
                applied.set(Some(enqueue));
                Ok(crate::task_lease::LeaseCompletion::Applied {
                    response_json: serde_json::json!({"ok": code == "curator_selection_enqueued", "code": code})
                        .to_string(),
                })
            },
        );
        match (outcome, applied.take()) {
            (Ok(crate::LeaseCompleteOutcome::Applied { .. }), Some(enqueue)) => Ok(enqueue),
            (Ok(crate::LeaseCompleteOutcome::Replayed { .. }), _) => Ok(EnqueueOutcome::Replayed),
            (Ok(crate::LeaseCompleteOutcome::Conflict { kind }), _) => {
                Err(CuratorJobError::Refused(if kind == "invalid" {
                    CuratorJobRefusal::InvalidRequest
                } else {
                    CuratorJobRefusal::SlotConflict
                }))
            }
            (Err(_), Some(deferred @ EnqueueOutcome::Deferred(_))) => Ok(deferred),
            (Err(error), _) => Err(CuratorJobError::Store(error)),
            (Ok(crate::LeaseCompleteOutcome::Applied { .. }), None) => {
                Err(CuratorJobError::Refused(CuratorJobRefusal::InvalidRequest))
            }
        }
    }

    /// Completes a selection slot that froze nothing, advancing the slot's continuation to `cursor` in the same transaction; `response_json` is the slot's recorded reply.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_selection_slot(
        &self,
        project: &str,
        claim_id: &str,
        completion_id: &str,
        scheduler_instance: &str,
        slot: i64,
        slot_id: &str,
        cursor: Option<&str>,
        response_json: &str,
        now_ms: i64,
    ) -> Result<crate::LeaseCompleteOutcome, MemoryStoreError> {
        self.complete_task_lease(
            &crate::MEMORY_CLASSIFIER_TASK,
            project,
            claim_id,
            completion_id,
            scheduler_instance,
            slot,
            now_ms,
            |coordinated, _claim| {
                advance_selection_cursor_in_tx(coordinated.tx(), project, slot_id, cursor, now_ms)?;
                Ok(crate::task_lease::LeaseCompletion::Applied {
                    response_json: response_json.to_string(),
                })
            },
        )
    }

    /// The one frozen page a project holds, if any: the page a deferred or interrupted slot resumes with.
    pub fn frozen_selection_for_project(
        &self,
        project: &str,
    ) -> Result<Option<FrozenSelection>, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| {
                let key: Option<(String, String)> = conn
                    .query_row(
                        "SELECT slot_id, selection_attempt FROM curator_frozen_selections
                          WHERE project = ?1 AND state = 'frozen' ORDER BY created_at_ms LIMIT 1",
                        [project],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                match key {
                    Some((slot_id, attempt)) => load_selection(conn, project, &slot_id, &attempt),
                    None => Ok(None),
                }
            })
            .map_err(Into::into)
    }

    /// Where the project's selection under `slot_id` resumes: the cursor the last enqueued page or empty slot advanced to, or `None` at the start of a pass.
    pub fn selection_cursor(
        &self,
        project: &str,
        slot_id: &str,
    ) -> Result<Option<String>, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT cursor FROM curator_selection_cursors WHERE project = ?1 AND slot_id = ?2",
                    params![project, slot_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map(Option::flatten)
            })
            .map_err(Into::into)
    }

    pub fn lookup_curator_job(
        &self,
        project: &str,
        causal_identity: &str,
    ) -> Result<Option<CuratorJob>, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| load_curator_job(conn, project, causal_identity))
            .map_err(Into::into)
    }

    pub fn lookup_frozen_selection(
        &self,
        project: &str,
        slot_id: &str,
        selection_attempt: &str,
    ) -> Result<Option<FrozenSelection>, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| load_selection(conn, project, slot_id, selection_attempt))
            .map_err(Into::into)
    }

    /// Ready jobs of one project whose queue deadline is after `now_ms`, in deadline order, bounded by `limit`; the dispatcher's read. A row past its deadline is not dispatched even before the sweep expires it.
    pub fn ready_curator_jobs(
        &self,
        project: &str,
        limit: usize,
        now_ms: i64,
    ) -> Result<Vec<CuratorJob>, MemoryStoreError> {
        check_project(project)?;
        let limit = i64::try_from(limit.min(MAX_PENDING_CURATOR_JOBS_PER_PROJECT)).unwrap_or(0);
        self.inner
            .with_conn(|conn| {
                let mut statement = conn.prepare_cached(&format!(
                    "SELECT {JOB_COLUMNS} FROM curator_jobs
                     WHERE project = ?1 AND state = 'ready' AND queue_deadline_ms > ?3
                     ORDER BY queue_deadline_ms, causal_identity LIMIT ?2"
                ))?;
                let rows = statement.query_map(params![project, limit, now_ms], job_from_row)?;
                rows.collect()
            })
            .map_err(Into::into)
    }

    pub fn curator_headroom(&self, project: &str) -> Result<CuratorHeadroom, MemoryStoreError> {
        check_project(project)?;
        self.inner
            .with_conn(|conn| {
                let project_bytes = metadata_bytes(conn, Some(project))?;
                let host_bytes = metadata_bytes(conn, None)?;
                Ok(CuratorHeadroom {
                    pending_jobs: pending_jobs(conn, Some(project))?,
                    host_pending_jobs: pending_jobs(conn, None)?,
                    frozen_pages: frozen_pages(conn, Some(project))?,
                    host_frozen_pages: frozen_pages(conn, None)?,
                    project_metadata_bytes: project_bytes,
                    host_metadata_bytes: host_bytes,
                    project_metadata_remaining: MAX_CURATOR_METADATA_BYTES_PER_PROJECT
                        .saturating_sub(project_bytes),
                    host_metadata_remaining: MAX_CURATOR_METADATA_BYTES_PER_HOST
                        .saturating_sub(host_bytes),
                })
            })
            .map_err(Into::into)
    }
}
