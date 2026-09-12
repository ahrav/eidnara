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
use super::source_descriptor::{
    SOURCE_DESCRIPTOR_KIND, SourceDescriptorDetail, descriptor_object_id, reencoded_identity,
    stored_detail,
};
use super::{CachedSql, KernelError, KernelStore, Sensitivity, map_sqlite};

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

/// The complete canonical descriptor validated while the writer is held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentInputDescriptor {
    pub object_id: String,
    pub source_revision: i64,
    pub detail: SourceDescriptorDetail,
    pub domain_id: String,
    pub sensitivity: Sensitivity,
    pub created_commit_seq: i64,
}

/// A stale verdict that retains the writer until the consumer finishes acting on it.
#[derive(Debug)]
pub struct StaleCurrentInput<'a> {
    reason: StaleInput,
    _writer: MutexGuard<'a, Connection>,
    tip: i64,
    database_incarnation_id: String,
}

impl StaleCurrentInput<'_> {
    pub fn reason(&self) -> &StaleInput {
        &self.reason
    }

    pub fn tip(&self) -> i64 {
        self.tip
    }

    pub fn database_incarnation_id(&self) -> &str {
        &self.database_incarnation_id
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
    descriptor: CurrentInputDescriptor,
}

impl CurrentInputGuard<'_> {
    /// The commit-log tip the descriptor was judged current at; no writer through this kernel instance can move it while the guard lives.
    pub fn tip(&self) -> i64 {
        self.tip
    }

    pub fn database_incarnation_id(&self) -> &str {
        &self.database_incarnation_id
    }

    pub fn descriptor(&self) -> &CurrentInputDescriptor {
        &self.descriptor
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
    /// `Ok(Err(stale))` names the first disagreement and retains the writer so the consumer can act on that verdict without a canonical mutation intervening.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Deadline`] when the writer stays held past `deadline`, [`KernelError::InvalidInput`] for an expectation outside the eligibility bounds, [`KernelError::FenceLost`] for a superseded writer, [`KernelError::CorruptCanonicalRow`] for a descriptor row that cannot be decoded, contradicts its own identity, or is missing behind a live registry row, and [`KernelError::Busy`] or [`KernelError::Io`] for a store that cannot be read.
    pub fn guard_current_input(
        &self,
        expected: &CurrentInputExpectation,
        eligibility: EligibilityBinding<'_>,
        deadline: Instant,
    ) -> Result<Result<CurrentInputGuard<'_>, StaleCurrentInput<'_>>, KernelError> {
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
        Ok(match judged.outcome {
            Ok(descriptor) => Ok(CurrentInputGuard {
                _writer: writer,
                tip: judged.tip,
                database_incarnation_id: judged.database_incarnation_id,
                descriptor,
            }),
            Err(reason) => Err(StaleCurrentInput {
                reason,
                _writer: writer,
                tip: judged.tip,
                database_incarnation_id: judged.database_incarnation_id,
            }),
        })
    }
}

struct RevalidatedInput {
    tip: i64,
    database_incarnation_id: String,
    outcome: Result<CurrentInputDescriptor, StaleInput>,
}

struct StoredDescriptorRow {
    object_kind: String,
    domain_id: String,
    source_kind: String,
    source_id: String,
    source_revision: i64,
    registry_created: i64,
    registry_invalidated: Option<i64>,
    registry_sensitivity: String,
    observation_kind: String,
    payload: Vec<u8>,
    evidence_id: Option<String>,
    observation_created: i64,
    observation_invalidated: Option<i64>,
    observation_sensitivity: String,
}

impl StoredDescriptorRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            object_kind: row.get(0)?,
            domain_id: row.get(1)?,
            source_kind: row.get(2)?,
            source_id: row.get(3)?,
            source_revision: row.get(4)?,
            registry_created: row.get(5)?,
            registry_invalidated: row.get(6)?,
            registry_sensitivity: row.get(7)?,
            observation_kind: row.get(8)?,
            payload: row.get(9)?,
            evidence_id: row.get(10)?,
            observation_created: row.get(11)?,
            observation_invalidated: row.get(12)?,
            observation_sensitivity: row.get(13)?,
        })
    }

    fn into_validated_descriptor(
        self,
        expected: &CurrentInputExpectation,
    ) -> Result<CurrentInputDescriptor, KernelError> {
        let detail = stored_detail(&self.payload)?;
        let encoded = reencoded_identity(&detail).ok_or(KernelError::CorruptCanonicalRow)?;
        if self.object_kind != "observation"
            || self.observation_kind != SOURCE_DESCRIPTOR_KIND
            || self.source_kind != detail.class
            || self.source_id != detail.lineage_id
            || self.source_revision.to_string() != detail.revision
            || self.registry_created != self.observation_created
            || self.registry_invalidated != self.observation_invalidated
            || self.registry_sensitivity != self.observation_sensitivity
            || descriptor_object_id(&detail.lineage_id, &detail.revision) != expected.object_id
            || self.evidence_id.as_deref() != Some(detail.evidence_id.as_str())
            || (detail.span.is_none() && detail.payload_id != detail.artifact_digest)
            || detail.source_policy.validate_for(encoded.class).is_err()
        {
            return Err(KernelError::CorruptCanonicalRow);
        }
        Ok(CurrentInputDescriptor {
            object_id: expected.object_id.clone(),
            source_revision: self.source_revision,
            detail,
            domain_id: self.domain_id,
            sensitivity: Sensitivity::from_stored(&self.observation_sensitivity),
            created_commit_seq: self.observation_created,
        })
    }
}

/// One deferred read on the writer connection: the eligibility verdict and the stored descriptor detail at the same tip.
/// The transaction ends before the function returns, so the guard never carries an open kernel transaction.
fn revalidate(
    writer: &mut Connection,
    expected: &CurrentInputExpectation,
    candidate: &EligibilityCandidate,
    eligibility: EligibilityBinding<'_>,
) -> Result<RevalidatedInput, KernelError> {
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
    let verdicts = judge_in_tx(
        &tx,
        tip,
        eligibility.project,
        eligibility.destination,
        std::slice::from_ref(candidate),
    )?;
    let (early_stale, eligibility_stale) =
        match verdicts.first().ok_or(KernelError::CorruptCanonicalRow)? {
            EligibilityVerdict::Ok => (None, None),
            EligibilityVerdict::Retracted => (Some(StaleInput::Retracted), None),
            EligibilityVerdict::Superseded => (Some(StaleInput::Superseded), None),
            EligibilityVerdict::Stale => {
                let current: i64 = tx
                    .query_row_cached(
                        "SELECT source_revision FROM object_registry WHERE object_id=?1",
                        [&expected.object_id],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite)?;
                (Some(StaleInput::RevisionChanged { current }), None)
            }
            verdict => (None, Some(StaleInput::Ineligible(*verdict))),
        };
    let stored = tx
        .query_row_cached(
            "SELECT r.object_kind,r.domain_id,r.source_kind,r.source_id,r.source_revision,
                    r.created_commit_seq,r.invalidated_commit_seq,r.sensitivity_class,
                    o.observation_kind,o.observation_payload,o.evidence_id,
                    o.created_commit_seq,o.invalidated_commit_seq,o.sensitivity_class
             FROM object_registry r JOIN observations o ON o.object_id=r.object_id
             WHERE r.object_id=?1",
            [&expected.object_id],
            StoredDescriptorRow::from_row,
        )
        .optional()
        .map_err(map_sqlite)?;
    let Some(stored) = stored else {
        if let Some(stale) = early_stale {
            return Ok(RevalidatedInput {
                tip,
                database_incarnation_id,
                outcome: Err(stale),
            });
        }
        return Err(KernelError::CorruptCanonicalRow);
    };
    let descriptor = stored.into_validated_descriptor(expected)?;
    if let Some(stale) = early_stale {
        return Ok(RevalidatedInput {
            tip,
            database_incarnation_id,
            outcome: Err(stale),
        });
    }
    let evidence: Option<(String, Option<i64>)> = tx
        .query_row_cached(
            "SELECT artifact_digest,invalidated_commit_seq
             FROM evidence_meta WHERE evidence_id=?1",
            params![descriptor.detail.evidence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    match evidence {
        Some((digest, None)) if digest == descriptor.detail.artifact_digest => {}
        Some((_, Some(_))) => {
            return Ok(RevalidatedInput {
                tip,
                database_incarnation_id,
                outcome: Err(StaleInput::Retracted),
            });
        }
        Some((_, None)) | None => return Err(KernelError::CorruptCanonicalRow),
    }
    if let Some(reason) = eligibility_stale {
        return Ok(RevalidatedInput {
            tip,
            database_incarnation_id,
            outcome: Err(reason),
        });
    }
    if descriptor.detail.occurrence_id != expected.occurrence_id
        || descriptor.detail.payload_id != expected.payload_id
        || descriptor.detail.artifact_digest != expected.artifact_digest
    {
        return Ok(RevalidatedInput {
            tip,
            database_incarnation_id,
            outcome: Err(StaleInput::InputChanged),
        });
    }
    Ok(RevalidatedInput {
        tip,
        database_incarnation_id,
        outcome: Ok(descriptor),
    })
}
