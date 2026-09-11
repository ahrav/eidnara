//! Drives the search projection from its durable local prefix toward one fixed canonical target and acknowledges kernel progress behind it.
//!
//! Each episode captures one target and processes windows until it reaches or refuses that target.
//! A window is one page of complete commits for the search consumer: the capture hold is extended over it, its source delta is exported, the delta commits as one local batch, and the window is acknowledged.
//! The kernel is never asked to acknowledge a commit the projection has not durably applied.
//! The local transaction is committed and released before the kernel writer is taken, so the two databases never hold transactions at the same time.
//!
//! A refusal ends the episode without moving either checkpoint.
//! A window that carries an artifact deletion is refused, because the descriptor export carries no tombstone for it and acknowledging it would report the deletion as propagated.
//! An unknown local commit outcome is reconciled from the projection's durable rows.
//! An unknown acknowledgement outcome is reconciled from the kernel's durable consumer checkpoint.
//! Integrity and storage failures quarantine the projection, so no acknowledgement can rest on a projection whose contents are in doubt.

use std::num::NonZeroUsize;

use kernel::{
    ARTIFACT_DELETION_SOURCE_KIND, CommitPageBounds, CommitReadError, CommitReadRequest,
    CompleteCommit, ExportWindow, KernelError, KernelStore, PageEnd, SourceExportError,
    SourceHoldAdmission, SourceHoldBinding, SourceHoldError, SourcePageBounds, SourceRow,
};
use retrieval::ProjectionError;
use retrieval::batch::{
    BatchBounds, BatchStatus, MutationIdentity, ProjectionBatch, ProjectionCheckpoint,
    batch_from_rows, read_checkpoint, row_identities,
};

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
/// Every bound is judged before the work it bounds materializes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeBounds {
    pub commits: CommitPageBounds,
    pub hold_admission: SourceHoldAdmission,
    pub source_page: SourcePageBounds,
    /// Source pages one window's delta may span before the episode refuses it.
    pub max_source_pages: NonZeroUsize,
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
    /// The local transaction has ended and its connection is released; the projection holds the whole window or none of it.
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

/// Why an episode stopped short of its target.
/// Nothing durable moved for the window that was refused; the next episode starts from the same prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpisodeEnd {
    /// Every commit through the captured target is applied and acknowledged.
    ReachedTarget,
    Blocked(Blocked),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeReport {
    pub target: i64,
    /// The kernel consumer checkpoint when the episode ended.
    pub acknowledged_through: i64,
    pub batches_applied: usize,
    pub commits_consumed: usize,
    pub end: EpisodeEnd,
}

#[derive(Debug, thiserror::Error)]
pub enum CatchUpError {
    #[error("the search projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
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
    /// Acknowledges while the local write transaction is still open.
    AcknowledgeInsideLocalTransaction,
}

/// Runs bounded catch-up episodes for one projection against one kernel.
pub struct SearchCatchUp<'a> {
    kernel: &'a KernelStore,
    projection: &'a SearchProjection,
    fault: Option<EpisodeFault>,
}

/// Why one step ended the episode: a refusal that moved nothing, or a failure the caller must see.
enum Stop {
    Blocked(Blocked),
    Failed(CatchUpError),
}

impl From<Blocked> for Stop {
    fn from(blocked: Blocked) -> Self {
        Stop::Blocked(blocked)
    }
}

impl From<CatchUpError> for Stop {
    fn from(error: CatchUpError) -> Self {
        Stop::Failed(error)
    }
}

impl From<KernelError> for Stop {
    fn from(error: KernelError) -> Self {
        Stop::Failed(error.into())
    }
}

impl<'a> SearchCatchUp<'a> {
    pub fn new(kernel: &'a KernelStore, projection: &'a SearchProjection) -> Self {
        Self {
            kernel,
            projection,
            fault: None,
        }
    }

    pub fn quarantine(&self) -> Option<Quarantine> {
        self.projection.quarantine()
    }

    /// Captures a target, brings the kernel checkpoint up to the durable local prefix, then applies and acknowledges one window at a time until the target is reached or a step refuses.
    ///
    /// `now` is Unix-epoch milliseconds: it is recorded as the projection rows' write time and the acknowledgement's `updated_at`, and the export judges hold expiry against it.
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
        self.run_episode_inner(consumer, bounds, now, observer)
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
        self.run_episode_inner(consumer, bounds, now, observer)
    }

    fn run_episode_inner(
        &mut self,
        consumer: &CatchUpConsumer,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<EpisodeReport, CatchUpError> {
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(CatchUpError::Quarantined(quarantine));
        }
        let target = self.kernel.capture_commit_read_target()?;
        let mut report = EpisodeReport {
            target: target.through_commit,
            acknowledged_through: 0,
            batches_applied: 0,
            commits_consumed: 0,
            end: EpisodeEnd::ReachedTarget,
        };
        match self.drive(
            consumer,
            bounds,
            now,
            observer,
            target.incarnation,
            &mut report,
        ) {
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
        incarnation: kernel::CommitReadIncarnation,
        report: &mut EpisodeReport,
    ) -> Result<(), Stop> {
        if now < 0 {
            return Err(Blocked::NegativeTime { now }.into());
        }
        report.acknowledged_through = self
            .kernel
            .outbox_consumer_checkpoint(&consumer.binding.consumer_id)?
            .ok_or(Blocked::Read(CommitReadError::UnknownConsumer))?;
        let checkpoint = self.local_prefix(consumer)?;
        let local = checkpoint.checkpoint_commit_seq;
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
            let hold = self
                .kernel
                .extend_source_hold(
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
        let mut after = local;
        while after < report.target {
            let page = self
                .kernel
                .read_complete_commits(
                    &CommitReadRequest {
                        consumer_id: consumer.binding.consumer_id.clone(),
                        incarnation,
                        after_commit: after,
                        through_commit: report.target,
                    },
                    bounds.commits,
                )
                .map_err(|error| match error {
                    CommitReadError::Kernel(error) => Stop::from(error),
                    error => Blocked::Read(error).into(),
                })?;
            let last = match page.end {
                PageEnd::Oversized {
                    commit_seq,
                    rows,
                    payload_bytes,
                } => {
                    return Err(Blocked::OversizedCommit {
                        commit_seq,
                        rows,
                        payload_bytes,
                    }
                    .into());
                }
                PageEnd::Exhausted => true,
                PageEnd::Deferred { .. } => false,
            };
            // The target is a commit the kernel had at capture, so an empty page or an exhausted page below it means the commit log lost commits.
            if page.commits.is_empty() || (last && page.through < report.target) {
                return Err(Blocked::TargetUnreachable {
                    after,
                    target: report.target,
                }
                .into());
            }
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
        }
        Ok(())
    }

    /// Reads the projection's checkpoint; the checkpoint's hold id must match `consumer.hold_id`.
    fn local_prefix(&mut self, consumer: &CatchUpConsumer) -> Result<ProjectionCheckpoint, Stop> {
        let read = self
            .projection
            .read(|conn| read_checkpoint(conn, &consumer.kernel_incarnation_id));
        let checkpoint = match read {
            Ok(Some(checkpoint)) => checkpoint,
            Ok(None) => return Err(Blocked::NoLocalBaseline.into()),
            Err(SearchProjectionError::Projection(ProjectionError::IdentityMismatch)) => {
                return Err(Blocked::ProjectionIdentity.into());
            }
            Err(error) => return Err(self.quarantine_from(error).into()),
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
        let hold = self
            .kernel
            .extend_source_hold(
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
        match self.commit_batch(consumer, &batch, bounds, now, observer) {
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
                if classify_store_failure(&error) == StoreFailure::Superseded =>
            {
                Err(Blocked::ProjectionIdentity.into())
            }
            // The store failed somewhere between BEGIN and COMMIT; the durable rows, not the error, say whether COMMIT took effect.
            Err(SearchProjectionError::Store(_)) => match self.projection.batch_status(&batch) {
                Ok(BatchStatus::Applied) => Ok(()),
                Ok(BatchStatus::NotApplied) => Err(Blocked::LocalCommitUnresolved.into()),
                Err(error) => Err(self.quarantine_from(error).into()),
            },
        }
    }

    /// Runs the batch in one fenced local transaction and reports [`EpisodeEvent::LocalReleased`] once that transaction is over.
    fn commit_batch(
        &self,
        consumer: &CatchUpConsumer,
        batch: &ProjectionBatch<'_>,
        bounds: &EpisodeBounds,
        now: i64,
        observer: &mut dyn FnMut(EpisodeEvent),
    ) -> Result<(), SearchProjectionError> {
        let through = batch.identity.through_commit_seq;
        let kernel = self.kernel;
        let acknowledge_inside =
            self.fault == Some(EpisodeFault::AcknowledgeInsideLocalTransaction);
        let applied = self.projection.write(|conn| {
            retrieval::batch::apply_batch(conn, batch, bounds.batch, now)?;
            observer(EpisodeEvent::LocalStaged { through });
            if acknowledge_inside {
                // The control only needs the kernel writer taken inside the local transaction; its outcome is not this batch's.
                observer(EpisodeEvent::AcknowledgementRequested { through });
                let _ = kernel.acknowledge_through_source_hold(
                    &consumer.binding,
                    &consumer.hold_id,
                    through,
                    now,
                );
            }
            Ok(())
        });
        observer(EpisodeEvent::LocalReleased { through });
        if self.fault == Some(EpisodeFault::LoseLocalCommitReply) && applied.is_ok() {
            return Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_string(),
            )));
        }
        applied
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
        for _ in 0..bounds.max_source_pages.get() {
            let page = self
                .kernel
                .export_source_page(
                    &consumer.binding,
                    &consumer.hold_id,
                    now,
                    ExportWindow::Delta { after, through },
                    cursor.as_ref(),
                    bounds.source_page,
                )
                .map_err(|error| match error {
                    SourceExportError::Kernel(error) => Stop::from(error),
                    error => Blocked::Export(error).into(),
                })?;
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
        let mut acknowledged = self.kernel.acknowledge_through_source_hold(
            &consumer.binding,
            &consumer.hold_id,
            through,
            now,
        );
        if self.fault == Some(EpisodeFault::LoseAcknowledgementReply) && acknowledged.is_ok() {
            acknowledged = Err(SourceHoldError::Kernel(KernelError::Io));
        }
        match acknowledged {
            Ok(()) => {}
            Err(SourceHoldError::Kernel(error)) if outcome_unknown(error) => {
                let kernel_checkpoint = self
                    .kernel
                    .outbox_consumer_checkpoint(&consumer.binding.consumer_id)?;
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

    fn quarantine_from(&mut self, error: SearchProjectionError) -> CatchUpError {
        if let SearchProjectionError::Quarantined(quarantine) = &error {
            return CatchUpError::Quarantined(quarantine.clone());
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
        self.enter_quarantine(kind, &error)
    }

    /// Another writer can quarantine the projection while an episode runs, so
    /// `refuse_if_quarantined` re-reads shared state.
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

/// An acknowledgement that failed this way may still have committed, because the failure can strike after the kernel's COMMIT or while waiting for its writer, so the durable checkpoint decides.
/// This is wider than [`KernelError::is_retryable`]: `Io` and `Deadline` are not safe to blindly retry, but they leave the outcome unknown all the same.
/// Every other kernel error is raised before the write begins or reports a definite rollback.
fn outcome_unknown(error: KernelError) -> bool {
    matches!(
        error,
        KernelError::Busy | KernelError::Held | KernelError::Io | KernelError::Deadline
    )
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
        | ProjectionError::CorruptRow => Refusal::Integrity,
        ProjectionError::Sqlite(_) => Refusal::Storage,
    }
}
