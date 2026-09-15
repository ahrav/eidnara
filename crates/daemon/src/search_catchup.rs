//! Drives the search projection from its durable local prefix toward one fixed canonical target and acknowledges kernel progress behind it.
//!
//! Each episode captures one target and processes windows until it reaches or refuses that target.
//! A window is one page of complete commits for the search consumer: the capture hold is extended over it, its source delta is exported, the delta commits as one local batch, and the window is acknowledged.
//! The kernel is never asked to acknowledge a commit the projection has not durably applied.
//! The local transaction is committed and released before the kernel writer is taken, so the two databases never hold transactions at the same time.
//!
//! A window that carries an artifact deletion is refused, because the descriptor export carries no tombstone for it and acknowledging it would report the deletion as propagated.
//! An unknown local commit outcome is reconciled from the projection's durable rows.
//! An unknown acknowledgement outcome is reconciled from the kernel's durable consumer checkpoint.
//! Integrity and storage failures quarantine the projection, so no acknowledgement can rest on a projection whose contents are in doubt.

use std::num::{NonZeroU64, NonZeroUsize};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{
    ARTIFACT_DELETION_SOURCE_KIND, CommitPageBounds, CommitReadError, CompleteCommit, ExportWindow,
    KernelError, KernelStore, PageBound, SourceExportError, SourceHoldAdmission, SourceHoldBinding,
    SourceHoldError, SourcePageBounds, SourceRow,
};
use retrieval::ProjectionError;
use retrieval::batch::{
    BatchBounds, BatchStatus, MutationIdentity, ProjectionBatch, ProjectionCheckpoint,
    batch_from_rows, read_checkpoint, row_identities,
};

use crate::commit_stream::{CommitStreamBlocked, CommitWalk, drive_commit_pages, outcome_unknown};
use crate::search_projection::{
    SearchProjection, SearchProjectionError, StoreFailure, classify_store_failure,
};
pub use crate::search_writer::{Quarantine, QuarantineKind};

/// The registered consumer, its capture hold, and the projection identity an episode acts under.
/// The hold must be the one the projection's durable checkpoint records; the episode refuses any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchUpConsumer {
    pub binding: SourceHoldBinding,
    pub hold_id: String,
    pub kernel_incarnation_id: String,
    /// The registered vector generation new dense-eligible rows queue work under, or `None` for a lexical-only projection.
    pub generation_id: Option<String>,
}

/// Bounds one episode's reads, hold extensions, exports, and local batches.
/// Export admission bounds retained rows and text before decoding; batch admission bounds mutations before writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeBounds {
    pub commits: CommitPageBounds,
    pub hold_admission: SourceHoldAdmission,
    pub source_page: SourcePageBounds,
    /// Source pages one window's delta may span before the episode refuses it.
    pub max_source_pages: NonZeroUsize,
    /// Total encoded work across a window. An artifact is charged again when another page reads it.
    pub max_source_encoded_bytes: NonZeroU64,
    /// The smaller record/mutation limit conservatively caps retained source rows, including invalidation-only rows.
    /// `max_source_bytes` additionally caps retained text bytes, not metadata or live heap.
    /// A created-and-invalidated row contributes two mutations; the full mutation count is admitted before local writes, not before decoding.
    pub batch: BatchBounds,
}

/// A boundary the episode crosses, reported in the order it crosses them for one window.
/// The observer runs on the episode's thread; at [`Self::LocalStaged`] it runs while the projection connection is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeEvent {
    /// The kernel writer is about to be taken to extend the hold over this window.
    HoldExtensionRequested {
        through: i64,
    },
    /// The batch's statements have run and the local transaction is still open.
    LocalStaged {
        through: i64,
    },
    /// The write attempt ended and no transaction remains; the projection holds the whole window or none of it, even if no write lock was acquired.
    LocalReleased {
        through: i64,
    },
    /// The kernel writer is about to be taken to acknowledge this window.
    AcknowledgementRequested {
        through: i64,
    },
    Acknowledged {
        through: i64,
    },
}

/// Cancellation can leave a committed local prefix awaiting acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// The shared operation budget was cancelled or its original deadline elapsed. This can follow the final acknowledgement; report counters retain that completed progress.
    Cancelled,
    /// The kernel rejects negative episode times at acknowledgement, so the
    /// episode refuses them before persisting an unacknowledgeable window.
    NegativeTime {
        now: i64,
    },
    /// No batch has ever committed, so there is no prefix to extend.
    NoLocalBaseline,
    /// The projection's checkpoint names another hold.
    BaselineMismatch {
        hold_id: String,
        snapshot_commit_seq: i64,
    },
    /// The installed projection identity is missing or names another kernel.
    ProjectionIdentity,
    /// The kernel has acknowledged commits the projection never applied, which no later window can repair.
    AcknowledgedBeyondLocalPrefix {
        local: i64,
        acknowledged: i64,
    },
    Read(CommitReadError),
    OversizedCommit {
        commit_seq: i64,
        rows: usize,
        payload_bytes: u64,
    },
    /// The commit log holds no complete commit between the applied prefix and the captured target.
    TargetUnreachable {
        after: i64,
        target: i64,
    },
    /// The window holds an artifact deletion, and the projection has no way to tombstone the deleted occurrences.
    /// Acknowledging it would satisfy the consumer's deletion barrier while the deleted text is still served.
    DeletionUnpropagated {
        commit_seq: i64,
    },
    HoldExtension(SourceHoldError),
    Export(SourceExportError),
    SourcePagesExceeded {
        through: i64,
    },
    /// The window cannot admit another source row within its aggregate allowance.
    SourceCapacityExceeded {
        through: i64,
        bound: &'static str,
    },
    /// The batch was refused before anything of it became durable.
    Admission(ProjectionError),
    /// The local commit reply was lost and the durable rows show the window was not applied.
    LocalCommitUnresolved,
    Acknowledgement(SourceHoldError),
    /// The acknowledgement reply was lost and the kernel's durable checkpoint still sits below the window the local prefix already covers.
    AcknowledgementUnresolved {
        through: i64,
        kernel_checkpoint: Option<i64>,
    },
    /// The acknowledgement may have committed, but its checkpoint could not be read back.
    AcknowledgementReconciliationFailed {
        through: i64,
        error: KernelError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpisodeEnd {
    /// Every commit through the captured target is applied and acknowledged.
    ReachedTarget,
    Blocked(Blocked),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeReport {
    /// `target` is `0` when the episode is refused before capture.
    pub target: i64,
    /// The last confirmed kernel checkpoint; zero if refusal occurs before the checkpoint read.
    /// `acknowledged_through` can trail the kernel checkpoint when its reconciliation read fails.
    pub acknowledged_through: i64,
    pub batches_applied: usize,
    pub commits_consumed: usize,
    pub end: EpisodeEnd,
}

#[derive(Debug, thiserror::Error)]
pub enum CatchUpError {
    #[error("the search projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    #[error("the search projection writer was fenced before catch-up")]
    ProjectionFenced,
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

/// Which reply an episode loses, or which order it violates, so a test can watch the reconciliation and the ordering observation discriminate.
/// Only the test-support entry point can set one; a production build never injects a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeFault {
    /// The batch commits, then its reply arrives as a store failure whose effect is unknown.
    LoseLocalCommitReply,
    /// The acknowledgement commits, then its reply arrives as a kernel I/O failure.
    LoseAcknowledgementReply,
    /// The acknowledgement commits, its reply is lost, and cancellation prevents reconciliation.
    LoseAcknowledgementReplyAndCancel,
    /// Acknowledges while the local write transaction is still open.
    AcknowledgeInsideLocalTransaction,
}

/// Advances the wall-clock `now` captured at entry by monotonic `elapsed`, in milliseconds.
/// Rounds up so a partial millisecond cannot hide hold expiry from the kernel's `now >= expires_at` check.
pub(crate) fn audit_time(now: i64, elapsed: Duration) -> i64 {
    let elapsed = elapsed.as_nanos().div_ceil(1_000_000);
    now.saturating_add(i64::try_from(elapsed).unwrap_or(i64::MAX))
}

/// Runs bounded catch-up episodes for one projection against one kernel.
pub struct SearchCatchUp<'a> {
    kernel: &'a KernelStore,
    projection: &'a SearchProjection,
    fault: Option<EpisodeFault>,
    budget: EvalBudget,
    started: Instant,
}

enum Stop {
    Blocked(Blocked),
    Failed(CatchUpError),
}

impl From<Blocked> for Stop {
    fn from(blocked: Blocked) -> Self {
        Stop::Blocked(blocked)
    }
}

impl From<CommitStreamBlocked> for Blocked {
    fn from(blocked: CommitStreamBlocked) -> Self {
        match blocked {
            CommitStreamBlocked::NegativeTime { now } => Blocked::NegativeTime { now },
            CommitStreamBlocked::Read(error) => Blocked::Read(error),
            CommitStreamBlocked::OversizedCommit {
                commit_seq,
                rows,
                payload_bytes,
            } => Blocked::OversizedCommit {
                commit_seq,
                rows,
                payload_bytes,
            },
            CommitStreamBlocked::TargetUnreachable { after, target } => {
                Blocked::TargetUnreachable { after, target }
            }
        }
    }
}

impl From<CommitStreamBlocked> for Stop {
    fn from(blocked: CommitStreamBlocked) -> Self {
        Stop::Blocked(blocked.into())
    }
}

impl From<CatchUpError> for Stop {
    fn from(error: CatchUpError) -> Self {
        match error {
            CatchUpError::Kernel(error) => error.into(),
            error => Stop::Failed(error),
        }
    }
}

impl From<KernelError> for Stop {
    fn from(error: KernelError) -> Self {
        match error {
            KernelError::Deadline => Stop::Blocked(Blocked::Cancelled),
            error => Stop::Failed(error.into()),
        }
    }
}

impl<'a> SearchCatchUp<'a> {
    pub fn new(kernel: &'a KernelStore, projection: &'a SearchProjection) -> Self {
        Self {
            kernel,
            projection,
            fault: None,
            budget: EvalBudget::unbounded(),
            started: Instant::now(),
        }
    }

    /// Shares the caller's cancellation and absolute deadline across all episodes on this driver.
    /// Kernel acquisition and SQL scans share this budget; synchronous filesystem I/O can outlive it.
    pub fn with_budget(mut self, budget: EvalBudget) -> Self {
        self.budget = budget;
        self
    }

    pub fn quarantine(&self) -> Option<Quarantine> {
        self.projection.quarantine()
    }

    /// Captures a target, brings the kernel checkpoint up to the durable local prefix, then applies and acknowledges one window at a time until the target is reached or a step refuses.
    ///
    /// `now` is Unix-epoch milliseconds at entry. Elapsed monotonic time advances export checks and write/acknowledgement audit times; it never renews the operation deadline.
    ///
    /// # Errors
    ///
    /// Returns [`CatchUpError::Quarantined`] when a stored row contradicts the batch that wrote it, or when the projection store fails and its effect cannot be read back; the projection stays quarantined.
    /// Returns [`CatchUpError::Kernel`] when the kernel fails in a way that leaves no durable fact to reconcile against, such as a failed read or a hold extension that did not run.
    pub fn run_episode(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<EpisodeReport, CatchUpError> {
        self.fault = None;
        self.run_episode_inner(consumer, bounds, now, observer, None)
    }

    /// [`Self::run_episode`] toward `target`, a commit fixed before the episode instead of the tip captured by it.
    /// Commits after `target` are neither applied nor acknowledged, whatever the tip has moved to.
    /// A target whose incarnation differs from the captured tip's is refused with [`CommitReadError::IncarnationMismatch`].
    ///
    /// # Errors
    ///
    /// As [`Self::run_episode`].
    pub fn run_episode_toward(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        target: kernel::CommitReadTarget,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<EpisodeReport, CatchUpError> {
        self.fault = None;
        self.run_episode_inner(consumer, bounds, now, observer, Some(target))
    }

    /// [`Self::run_episode`] under one injected fault.
    #[cfg(feature = "test-support")]
    pub fn run_episode_with_fault_for_test(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
        fault: EpisodeFault,
    ) -> Result<EpisodeReport, CatchUpError> {
        self.fault = Some(fault);
        self.run_episode_inner(consumer, bounds, now, observer, None)
    }

    fn capture_target(&mut self) -> Result<kernel::CommitReadTarget, CatchUpError> {
        self.started = Instant::now();
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(CatchUpError::Quarantined(quarantine));
        }
        Ok(self
            .kernel
            .capture_commit_read_target_within_budget(&self.budget)?)
    }

    fn run_episode_inner(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
        fixed: Option<kernel::CommitReadTarget>,
    ) -> Result<EpisodeReport, CatchUpError> {
        let mut report = EpisodeReport {
            target: fixed.map_or(0, |target| target.through_commit),
            acknowledged_through: 0,
            batches_applied: 0,
            commits_consumed: 0,
            end: EpisodeEnd::ReachedTarget,
        };
        match self.drive(consumer, bounds, now, observer, fixed, &mut report) {
            Ok(()) => Ok(report),
            Err(Stop::Blocked(blocked)) => {
                report.end = EpisodeEnd::Blocked(blocked);
                Ok(report)
            }
            Err(Stop::Failed(error)) => Err(error),
        }
    }

    /// Advances `report` window by window; every early exit names why.
    fn drive(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
        fixed: Option<kernel::CommitReadTarget>,
        report: &mut EpisodeReport,
    ) -> Result<(), Stop> {
        if now < 0 {
            return Err(Blocked::NegativeTime { now }.into());
        }
        self.check_budget()?;
        let captured = self.capture_target()?;
        let target = fixed.unwrap_or(captured);
        report.target = target.through_commit;
        report.acknowledged_through = self
            .kernel
            .outbox_consumer_checkpoint_within_budget(&self.budget, &consumer.binding.consumer_id)?
            .ok_or(Blocked::Read(CommitReadError::UnknownConsumer))?;
        self.check_budget()?;
        if target.incarnation != captured.incarnation {
            return Err(Blocked::Read(CommitReadError::IncarnationMismatch).into());
        }
        if report.target < 0 {
            return Err(Blocked::Read(CommitReadError::InvalidRequest).into());
        }
        if report.target > captured.through_commit {
            return Err(Blocked::Read(CommitReadError::TargetBeyondTip).into());
        }
        let checkpoint = self.local_prefix(consumer)?;
        let local = checkpoint.checkpoint_commit_seq;
        if local > report.target {
            return Err(Blocked::Read(CommitReadError::InvalidRequest).into());
        }
        if report.acknowledged_through > local {
            return Err(Blocked::AcknowledgedBeyondLocalPrefix {
                local,
                acknowledged: report.acknowledged_through,
            }
            .into());
        }
        // A durable local prefix the kernel has not acknowledged is a lost reply from an earlier episode; the same prefix is acknowledged again.
        if report.acknowledged_through < local {
            observer(EpisodeEvent::HoldExtensionRequested { through: local });
            self.check_budget()?;
            let hold = self
                .kernel
                .extend_source_hold_within_budget(
                    &self.budget,
                    &consumer.binding,
                    &consumer.hold_id,
                    local,
                    bounds.hold_admission,
                )
                .map_err(|error| match error {
                    SourceHoldError::Kernel(error) => Stop::from(error),
                    error => Blocked::HoldExtension(error).into(),
                })?;
            if hold.snapshot != checkpoint.snapshot_commit_seq {
                return Err(self
                    .enter_quarantine(
                        QuarantineKind::Integrity,
                        &format!(
                            "the projection checkpoint claims baseline {}, but hold {} was \
                             captured at {}; acknowledging that claim would let kernel pruning \
                             advance over commits the projection never applied",
                            checkpoint.snapshot_commit_seq, consumer.hold_id, hold.snapshot
                        ),
                    )
                    .into());
            }
            self.acknowledge(consumer, local, now, observer)?;
            report.acknowledged_through = local;
        }
        let target = report.target;
        let mut after = local;
        self.check_budget()?;
        drive_commit_pages(
            self.kernel,
            CommitWalk {
                budget: Some(self.budget.clone()),
                consumer_id: &consumer.binding.consumer_id,
                incarnation: captured.incarnation,
                now,
                after: local,
                target,
                bounds: bounds.commits,
            },
            |page| {
                self.check_budget()?;
                // The export delivers creations, supersessions, and retirements; a deletion arrives only as a control row, and the batch it would need is not built here.
                if let Some(deletion) = first_deletion(&page.commits) {
                    return Err(Blocked::DeletionUnpropagated {
                        commit_seq: deletion,
                    }
                    .into());
                }
                let through = page.through;
                self.apply_window(consumer, bounds, after, through, now, observer)?;
                report.batches_applied += 1;
                report.commits_consumed += page.commits.len();
                self.acknowledge(consumer, through, now, observer)?;
                report.acknowledged_through = through;
                after = through;
                self.check_budget()?;
                Ok::<(), Stop>(())
            },
        )
    }

    /// Reads the projection's checkpoint; the checkpoint's hold id must match `consumer.hold_id`.
    fn local_prefix(&mut self, consumer: &CatchUpConsumer) -> Result<ProjectionCheckpoint, Stop> {
        let read = |conn: &storage::GuardedConn<'_>| {
            read_checkpoint(conn, &consumer.kernel_incarnation_id)
        };
        let read = match self.budget.deadline() {
            Some(deadline) => self.projection.read_within(deadline, read),
            None => self.projection.read(read),
        };
        let checkpoint = match read {
            Ok(Some(checkpoint)) => checkpoint,
            Ok(None) => return Err(Blocked::NoLocalBaseline.into()),
            Err(SearchProjectionError::Projection(ProjectionError::IdentityMismatch)) => {
                return Err(Blocked::ProjectionIdentity.into());
            }
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                return Err(Blocked::Cancelled.into());
            }
            Err(error) => return Err(self.stop_from_projection_error(error)),
        };
        if checkpoint.hold_id != consumer.hold_id {
            return Err(Blocked::BaselineMismatch {
                hold_id: checkpoint.hold_id,
                snapshot_commit_seq: checkpoint.snapshot_commit_seq,
            }
            .into());
        }
        Ok(checkpoint)
    }

    /// Extends the hold through `through`, exports the window's delta, and commits it as one batch.
    /// Returns only after the local transaction has ended and released its connection, whichever way it ended.
    fn apply_window(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        after: i64,
        through: i64,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<(), Stop> {
        self.refuse_if_quarantined()?;
        observer(EpisodeEvent::HoldExtensionRequested { through });
        self.check_budget()?;
        let hold = self
            .kernel
            .extend_source_hold_within_budget(
                &self.budget,
                &consumer.binding,
                &consumer.hold_id,
                through,
                bounds.hold_admission,
            )
            .map_err(|error| match error {
                SourceHoldError::Kernel(error) => Stop::from(error),
                error => Blocked::HoldExtension(error).into(),
            })?;
        let rows = self.export_window(consumer, bounds, now, after, through)?;
        let identities = row_identities(&rows);
        // The hold's snapshot is the checkpoint's snapshot: `local_prefix` matched the hold id, and a hold has one S.
        let identity = MutationIdentity {
            kernel_incarnation_id: consumer.kernel_incarnation_id.clone(),
            hold_id: consumer.hold_id.clone(),
            snapshot_commit_seq: hold.snapshot,
            through_commit_seq: through,
        };
        let batch = batch_from_rows(
            &rows,
            &identities,
            identity,
            consumer.generation_id.as_deref(),
        )
        .map_err(Blocked::Admission)?;
        self.commit_batch(consumer, &batch, bounds, now, observer)
    }

    /// Reports [`EpisodeEvent::LocalReleased`] after the write attempt ends, including a refused lock acquisition.
    fn commit_batch(
        &mut self,
        consumer: &CatchUpConsumer,
        batch: &ProjectionBatch<'_>,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<(), Stop> {
        self.check_budget()?;
        let through = batch.identity.through_commit_seq;
        let kernel = self.kernel;
        let acknowledge_inside =
            self.fault == Some(EpisodeFault::AcknowledgeInsideLocalTransaction);
        let mut cancelled = false;
        // The projection closure accepts only projection errors. Record cancellation separately so its rollback signal never becomes a quarantine cause.
        let mut check = || {
            if self.budget.is_exhausted() {
                cancelled = true;
                Err(ProjectionError::MutationConflict)
            } else {
                Ok(())
            }
        };
        let write = |conn: &storage::GuardedConn<'_>| {
            check()?;
            retrieval::batch::apply_batch(conn, batch, bounds.batch, self.audit_time(now))?;
            observer(EpisodeEvent::LocalStaged { through });
            check()?;
            if acknowledge_inside {
                // The control only needs the kernel writer taken inside the local transaction; its outcome is not this batch's.
                observer(EpisodeEvent::AcknowledgementRequested { through });
                let _ = kernel.acknowledge_through_source_hold_within_budget(
                    &self.budget,
                    &consumer.binding,
                    &consumer.hold_id,
                    through,
                    self.audit_time(now),
                );
            }
            Ok(())
        };
        let mut applied = match self.budget.deadline() {
            Some(deadline) => self.projection.write_within(deadline, write),
            None => self.projection.write(write),
        };
        observer(EpisodeEvent::LocalReleased { through });
        if cancelled
            || matches!(
                applied,
                Err(SearchProjectionError::Store(storage::StoreError::Deadline))
            )
        {
            self.refuse_if_quarantined()?;
            return Err(Blocked::Cancelled.into());
        }
        if self.fault == Some(EpisodeFault::LoseLocalCommitReply) && applied.is_ok() {
            applied = Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_string(),
            )));
        }
        match applied {
            Ok(()) => Ok(()),
            // The store returned before COMMIT, so the batch rolled back and nothing of it is durable.
            Err(SearchProjectionError::Projection(error)) => Err(match classify(&error) {
                // A batch writes no vectors, so an operator-repair refusal is unreachable here and, if it ever arrives, is a refusal with nothing durable.
                Refusal::Admission | Refusal::OperatorRepair => Blocked::Admission(error).into(),
                Refusal::Identity => Blocked::ProjectionIdentity.into(),
                Refusal::Integrity => self
                    .enter_quarantine(QuarantineKind::Integrity, &error)
                    .into(),
                Refusal::Storage => self
                    .enter_quarantine(QuarantineKind::Storage, &error)
                    .into(),
            }),
            Err(SearchProjectionError::Quarantined(quarantine)) => {
                Err(CatchUpError::Quarantined(quarantine).into())
            }
            Err(SearchProjectionError::Connection(error)) => Err(self
                .enter_quarantine(QuarantineKind::Integrity, &error)
                .into()),
            Err(SearchProjectionError::Store(error))
                if classify_store_failure(&error) == StoreFailure::Integrity =>
            {
                Err(self
                    .enter_quarantine(QuarantineKind::Integrity, &error)
                    .into())
            }
            Err(SearchProjectionError::Store(error))
                if classify_store_failure(&error) == StoreFailure::Rejected =>
            {
                Err(CatchUpError::ProjectionFenced.into())
            }
            // The store failed somewhere between BEGIN and COMMIT; the durable rows, not the error, say whether COMMIT took effect.
            Err(SearchProjectionError::Store(_)) => {
                self.check_budget()?;
                let status =
                    |conn: &storage::GuardedConn<'_>| retrieval::batch::batch_status(conn, batch);
                let status = match self.budget.deadline() {
                    Some(deadline) => self.projection.read_within(deadline, status),
                    None => self.projection.read(status),
                };
                match status {
                    Ok(BatchStatus::Applied) => Ok(()),
                    Ok(BatchStatus::NotApplied) => Err(Blocked::LocalCommitUnresolved.into()),
                    Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                        Err(Blocked::Cancelled.into())
                    }
                    Err(error) => Err(self.stop_from_projection_error(error)),
                }
            }
        }
    }

    /// Collects every page of the window's delta, since source pages are keyset-ordered and a partial inventory cannot justify a checkpoint.
    fn export_window(
        &self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        now: i64,
        after: i64,
        through: i64,
    ) -> Result<Vec<SourceRow>, Stop> {
        let mut rows = Vec::new();
        let mut cursor = None;
        let mut remaining_rows = bounds
            .batch
            .persist
            .max_records
            .get()
            .min(bounds.batch.max_local_mutations.get());
        let mut remaining_decoded = bounds.batch.max_source_bytes.get() as u64;
        let mut remaining_encoded = bounds.max_source_encoded_bytes.get();
        let capacity = |bound| Blocked::SourceCapacityExceeded { through, bound };
        for _ in 0..bounds.max_source_pages.get() {
            self.check_budget()?;
            // A spent byte allowance still admits rows that charge nothing, such as invalidation-only rows.
            // A one-byte page bound makes the kernel refuse any larger charging row at preflight; the charge check after the page refuses a one-byte row.
            let page_bounds = SourcePageBounds {
                max_rows: NonZeroUsize::new(remaining_rows)
                    .ok_or_else(|| capacity("rows"))?
                    .min(bounds.source_page.max_rows),
                max_decoded_bytes: NonZeroU64::new(remaining_decoded)
                    .unwrap_or(NonZeroU64::MIN)
                    .min(bounds.source_page.max_decoded_bytes),
                max_encoded_bytes: NonZeroU64::new(remaining_encoded)
                    .unwrap_or(NonZeroU64::MIN)
                    .min(bounds.source_page.max_encoded_bytes),
                max_row_bytes: bounds.source_page.max_row_bytes,
            };
            let page = self
                .kernel
                .export_source_page_within_budget(
                    &self.budget,
                    &consumer.binding,
                    &consumer.hold_id,
                    self.audit_time(now),
                    ExportWindow::Delta { after, through },
                    cursor.as_ref(),
                    page_bounds,
                )
                .map_err(|error| match error {
                    SourceExportError::Kernel(error) => Stop::from(error),
                    SourceExportError::OversizedRow { bound, bytes, .. } => {
                        let (standalone, dimension) = match bound {
                            PageBound::Row => (bounds.source_page.max_row_bytes.get(), "text"),
                            PageBound::Decoded => {
                                (bounds.source_page.max_decoded_bytes.get(), "text")
                            }
                            PageBound::Encoded => {
                                (bounds.source_page.max_encoded_bytes.get(), "encoded")
                            }
                        };
                        if bytes <= standalone {
                            capacity(dimension).into()
                        } else {
                            Blocked::Export(error).into()
                        }
                    }
                    error => Blocked::Export(error).into(),
                })?;
            self.check_budget()?;
            remaining_rows = remaining_rows
                .checked_sub(page.charge.rows)
                .ok_or_else(|| capacity("rows"))?;
            remaining_decoded = remaining_decoded
                .checked_sub(page.charge.decoded_bytes)
                .ok_or_else(|| capacity("text"))?;
            remaining_encoded = remaining_encoded
                .checked_sub(page.charge.encoded_bytes)
                .ok_or_else(|| capacity("encoded"))?;
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(rows),
            }
        }
        Err(Blocked::SourcePagesExceeded { through }.into())
    }

    /// Acknowledges `through` after the local transaction for it has released; a lost reply is reconciled from the kernel's durable checkpoint.
    fn acknowledge(
        &mut self,
        consumer: &CatchUpConsumer,
        through: i64,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<(), Stop> {
        self.refuse_if_quarantined()?;
        observer(EpisodeEvent::AcknowledgementRequested { through });
        self.refuse_if_quarantined()?;
        self.check_budget()?;
        let mut guard_cancelled = false;
        let mut acknowledged = self
            .kernel
            .acknowledge_through_source_hold_if_within_budget(
                &self.budget,
                &consumer.binding,
                &consumer.hold_id,
                through,
                self.audit_time(now),
                || {
                    let guard = self.projection.acknowledgement_guard()?;
                    if self.check_budget().is_err() {
                        guard_cancelled = true;
                        return None;
                    }
                    Some(guard)
                },
            );
        if matches!(
            self.fault,
            Some(
                EpisodeFault::LoseAcknowledgementReply
                    | EpisodeFault::LoseAcknowledgementReplyAndCancel
            )
        ) && matches!(acknowledged, Ok(true))
        {
            if self.fault == Some(EpisodeFault::LoseAcknowledgementReplyAndCancel) {
                self.budget.cancel();
            }
            acknowledged = Err(SourceHoldError::Kernel(KernelError::Io));
        }
        match acknowledged {
            Ok(true) => {}
            Ok(false) => {
                if guard_cancelled {
                    self.refuse_if_quarantined()?;
                    return Err(Blocked::Cancelled.into());
                }
                let quarantine = self
                    .projection
                    .quarantine()
                    .expect("quarantine intent must record its cause before publication");
                return Err(CatchUpError::Quarantined(quarantine).into());
            }
            Err(SourceHoldError::Kernel(error)) if outcome_unknown(error) => {
                let kernel_checkpoint = self
                    .kernel
                    .outbox_consumer_checkpoint_within_budget(
                        &self.budget,
                        &consumer.binding.consumer_id,
                    )
                    .map_err(|error| Blocked::AcknowledgementReconciliationFailed {
                        through,
                        error,
                    })?;
                if kernel_checkpoint.is_none_or(|checkpoint| checkpoint < through) {
                    return Err(Blocked::AcknowledgementUnresolved {
                        through,
                        kernel_checkpoint,
                    }
                    .into());
                }
            }
            Err(SourceHoldError::Kernel(error)) => return Err(error.into()),
            Err(error) => return Err(Blocked::Acknowledgement(error).into()),
        }
        observer(EpisodeEvent::Acknowledged { through });
        Ok(())
    }

    fn stop_from_projection_error(&mut self, error: SearchProjectionError) -> Stop {
        if let SearchProjectionError::Quarantined(quarantine) = &error {
            return CatchUpError::Quarantined(quarantine.clone()).into();
        }
        if matches!(
            &error,
            SearchProjectionError::Projection(error) if classify(error) == Refusal::Identity
        ) {
            return Blocked::ProjectionIdentity.into();
        }
        if matches!(
            &error,
            SearchProjectionError::Store(error)
                if classify_store_failure(error) == StoreFailure::Rejected
        ) {
            return CatchUpError::ProjectionFenced.into();
        }
        let kind = match &error {
            SearchProjectionError::Projection(error) if classify(error) == Refusal::Integrity => {
                QuarantineKind::Integrity
            }
            SearchProjectionError::Connection(_) => QuarantineKind::Integrity,
            SearchProjectionError::Store(error)
                if classify_store_failure(error) == StoreFailure::Integrity =>
            {
                QuarantineKind::Integrity
            }
            _ => QuarantineKind::Storage,
        };
        self.enter_quarantine(kind, &error).into()
    }

    fn check_budget(&self) -> Result<(), Blocked> {
        self.budget.check().map_err(|_| Blocked::Cancelled)
    }

    fn audit_time(&self, now: i64) -> i64 {
        audit_time(now, self.started.elapsed())
    }

    fn refuse_if_quarantined(&self) -> Result<(), Stop> {
        match self.projection.quarantine() {
            Some(quarantine) => Err(CatchUpError::Quarantined(quarantine).into()),
            None => Ok(()),
        }
    }

    fn enter_quarantine(
        &mut self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> CatchUpError {
        CatchUpError::Quarantined(self.projection.enter_quarantine(kind, error))
    }
}

fn first_deletion(commits: &[CompleteCommit]) -> Option<i64> {
    commits
        .iter()
        .find(|commit| {
            commit
                .rows
                .iter()
                .any(|row| row.source_kind == ARTIFACT_DELETION_SOURCE_KIND)
        })
        .map(|commit| commit.commit_seq)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The batch was judged and refused before any row was written.
    Admission,
    Identity,
    /// The input contradicts durable state in a way no automatic retry may resolve.
    OperatorRepair,
    /// Stored state contradicts the batch or itself.
    Integrity,
    /// A statement failed for a reason the error text alone does not classify.
    Storage,
}

pub(crate) fn classify(error: &ProjectionError) -> Refusal {
    match error {
        ProjectionError::Occurrence(_)
        | ProjectionError::OverBound { .. }
        | ProjectionError::TooManyRecords { .. }
        | ProjectionError::NonPositiveSequence { .. }
        | ProjectionError::NonPositiveTombstoneSequence { .. }
        | ProjectionError::TombstoneNotAfterCreation { .. }
        | ProjectionError::MutationConflict
        | ProjectionError::MalformedBatch
        | ProjectionError::BatchOverBound { .. }
        | ProjectionError::UnknownGeneration { .. }
        | ProjectionError::RetiredGeneration { .. }
        | ProjectionError::NoPendingWork { .. } => Refusal::Admission,
        ProjectionError::InvalidVector { .. } | ProjectionError::VectorConflict { .. } => {
            Refusal::OperatorRepair
        }
        ProjectionError::IdentityMismatch => Refusal::Identity,
        // An admitted invalidation names an occurrence at or below the durable checkpoint; a missing row contradicts the applied prefix.
        ProjectionError::UnknownOccurrence { .. }
        | ProjectionError::OccurrenceCollision { .. }
        | ProjectionError::PayloadCollision { .. }
        | ProjectionError::TombstoneCollision { .. }
        | ProjectionError::AssociationCollision { .. }
        | ProjectionError::ExtractionVersionMismatch { .. }
        | ProjectionError::CorruptRow => Refusal::Integrity,
        ProjectionError::Sqlite(_) => Refusal::Storage,
    }
}
