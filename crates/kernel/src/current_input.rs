//! Holds one kernel instance's writer while a consumer publishes work derived from one source descriptor.
//!
//! Every canonical mutation through that instance runs under the writer mutex: source publication and retirement, artifact classification, remediation, and restore.
//! Taking that mutex excludes those mutations for as long as the guard lives, and revalidating the descriptor under it makes the comparison and the exclusion one step.
//! The guard opens no kernel transaction, so the consumer may commit to its own store while holding it without two transactions ever overlapping.
//! A successor kernel instance can advance the durable writer fence while this process still holds its instance-local mutex. The daemon excludes that topology with its process-lifetime instance fence; standalone cross-store consumers must provide equivalent lifetime ownership.
//! Acquisition is bounded by a deadline, and the guard performs no work of its own: what the consumer does under it is the consumer's bound.

use std::sync::MutexGuard;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::cas::ArtifactDestination;
use super::eligibility::{
    EligibilityCandidate, EligibilityVerdict, ProjectScope, check_bounds, judge_in_tx,
};
use super::envelope::check_fence;
use super::open::{AcquireLimit, database_incarnation_id_via};
use super::source_descriptor::{descriptor_object_id, reencoded_identity, stored_detail};
use super::{CachedSql, KernelError, KernelStore, map_sqlite};

/// The descriptor a consumer believes it is publishing work for, as it read it when the work began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentInputExpectation {
    pub object_id: String,
    pub source_revision: i64,
    pub occurrence_id: String,
    pub payload_id: String,
    pub artifact_digest: String,
}

/// Which project and destination the eligibility verdict is judged for.
#[derive(Debug, Clone, Copy)]
pub struct EligibilityBinding<'a> {
    pub project: &'a ProjectScope,
    pub destination: ArtifactDestination,
}

/// How the current descriptor differs from the expectation; any difference makes the expected work obsolete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleInput {
    /// The object is gone, retired, or its evidence was invalidated.
    Retracted,
    /// A newer revision of the same lineage replaced the object.
    Superseded,
    /// The object is live at a revision other than the expected one; `current` is the revision it holds now.
    RevisionChanged { current: i64 },
    /// The descriptor no longer names the expected occurrence, payload, or artifact.
    InputChanged,
    /// The kernel no longer judges the object eligible for the destination.
    Ineligible(EligibilityVerdict),
}

/// Carries the incarnation from the same read transaction as the verdict, so a consumer can fence another store without reopening the kernel after the writer is released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleCurrentInput {
    reason: StaleInput,
    database_incarnation_id: String,
}

impl StaleCurrentInput {
    pub fn reason(&self) -> &StaleInput {
        &self.reason
    }

    pub fn into_parts(self) -> (StaleInput, String) {
        (self.reason, self.database_incarnation_id)
    }
}

/// One kernel instance's writer, held until dropped. No canonical mutation through that instance can begin while it lives.
///
/// The holder must call nothing on the [`KernelStore`] until the guard drops: the writer mutex is not reentrant, and a bounded kernel call would burn its whole deadline before failing.
/// The only lock order is kernel writer first, then the consumer's own store; code that holds its own store's writer must never wait for this guard.
#[derive(Debug)]
pub struct CurrentInputGuard<'a> {
    _writer: MutexGuard<'a, Connection>,
    tip: i64,
    database_incarnation_id: String,
}

impl CurrentInputGuard<'_> {
    /// The commit-log tip the descriptor was judged current at; no writer through this kernel instance can move it while the guard lives.
    pub fn tip(&self) -> i64 {
        self.tip
    }

    pub fn database_incarnation_id(&self) -> &str {
        &self.database_incarnation_id
    }

    /// Whether the held writer connection has a transaction open; the guard promises it never does.
    #[cfg(feature = "test-support")]
    pub fn holds_kernel_transaction_for_test(&self) -> bool {
        !self._writer.is_autocommit()
    }
}

impl KernelStore {
    /// Takes the kernel writer within `deadline` and, under it, judges whether `expected` still names the current, eligible descriptor.
    /// `Ok(Ok(guard))` means every field agreed at the moment the writer was taken and no canonical mutation through this kernel instance can begin until the guard drops.
    /// `Ok(Err(stale))` names the first disagreement and the database incarnation that judged it, and has already released the writer.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Deadline`] when the writer stays held past `deadline`, [`KernelError::InvalidInput`] for an expectation outside the eligibility bounds, [`KernelError::FenceLost`] for a superseded writer, [`KernelError::CorruptCanonicalRow`] for a descriptor row that cannot be decoded, contradicts its own identity, or is missing behind a live registry row, and [`KernelError::Busy`] or [`KernelError::Io`] for a store that cannot be read.
    pub fn guard_current_input(
        &self,
        expected: &CurrentInputExpectation,
        eligibility: EligibilityBinding<'_>,
        deadline: Instant,
    ) -> Result<Result<CurrentInputGuard<'_>, StaleCurrentInput>, KernelError> {
        let candidate = EligibilityCandidate {
            object_id: expected.object_id.clone(),
            source_revision: expected.source_revision,
            artifact_digest: Some(expected.artifact_digest.clone()),
        };
        check_bounds(std::slice::from_ref(&candidate))?;
        let mut writer = self.lock_writer_within(&AcquireLimit::until(deadline))?;
        check_fence(&writer, self.lease_epoch())?;
        let judged = revalidate(&mut writer, expected, &candidate, eligibility)?;
        // The revalidation transaction has ended; a connection still inside one would hand the consumer an open kernel transaction.
        if !writer.is_autocommit() {
            return Err(KernelError::Io);
        }
        Ok(
            judged.map(|(tip, database_incarnation_id)| CurrentInputGuard {
                _writer: writer,
                tip,
                database_incarnation_id,
            }),
        )
    }
}

/// One deferred read on the writer connection: the eligibility verdict and the stored descriptor detail at the same tip.
/// The transaction ends before the function returns, so the guard never carries an open kernel transaction.
fn revalidate(
    writer: &mut Connection,
    expected: &CurrentInputExpectation,
    candidate: &EligibilityCandidate,
    eligibility: EligibilityBinding<'_>,
) -> Result<Result<(i64, String), StaleCurrentInput>, KernelError> {
    let tx = writer
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite)?;
    let tip: i64 = tx
        .query_row_cached(
            "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    let database_incarnation_id = database_incarnation_id_via(&tx)?;
    let stale = |reason| StaleCurrentInput {
        reason,
        database_incarnation_id: database_incarnation_id.clone(),
    };
    let verdicts = judge_in_tx(
        &tx,
        tip,
        eligibility.project,
        eligibility.destination,
        std::slice::from_ref(candidate),
    )?;
    match verdicts.first().ok_or(KernelError::CorruptCanonicalRow)? {
        EligibilityVerdict::Ok => {}
        EligibilityVerdict::Retracted => return Ok(Err(stale(StaleInput::Retracted))),
        EligibilityVerdict::Superseded => return Ok(Err(stale(StaleInput::Superseded))),
        EligibilityVerdict::Stale => {
            let current: i64 = tx
                .query_row_cached(
                    "SELECT source_revision FROM object_registry WHERE object_id=?1",
                    [&expected.object_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;
            return Ok(Err(stale(StaleInput::RevisionChanged { current })));
        }
        verdict => return Ok(Err(stale(StaleInput::Ineligible(*verdict)))),
    }
    let (payload, evidence_id): (Vec<u8>, Option<String>) = tx
        .query_row_cached(
            "SELECT observation_payload, evidence_id FROM observations WHERE object_id=?1",
            [&expected.object_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or(KernelError::CorruptCanonicalRow)?;
    let detail = stored_detail(&payload)?;
    // Mismatches indicate canonical-row corruption, not staleness.
    if descriptor_object_id(&detail.lineage_id, &detail.revision) != expected.object_id
        || detail.revision != expected.source_revision.to_string()
        || evidence_id.as_deref() != Some(detail.evidence_id.as_str())
        || reencoded_identity(&detail).is_none()
        || (detail.span.is_none() && detail.payload_id != detail.artifact_digest)
    {
        return Err(KernelError::CorruptCanonicalRow);
    }
    let evidence: Option<(String, Option<i64>)> = tx
        .query_row_cached(
            "SELECT artifact_digest,invalidated_commit_seq
             FROM evidence_meta WHERE evidence_id=?1",
            params![detail.evidence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    match evidence {
        Some((digest, None)) if digest == detail.artifact_digest => {}
        Some((_, Some(_))) => return Ok(Err(stale(StaleInput::Retracted))),
        Some((_, None)) | None => return Err(KernelError::CorruptCanonicalRow),
    }
    if detail.occurrence_id != expected.occurrence_id
        || detail.payload_id != expected.payload_id
        || detail.artifact_digest != expected.artifact_digest
    {
        return Ok(Err(stale(StaleInput::InputChanged)));
    }
    Ok(Ok((tip, database_incarnation_id)))
}
