//! Publishes one validated vector into the search projection while the kernel's current-input guard excludes every canonical mutation of its source.
//!
//! The vector is validated before the guard is taken, the guard is taken without a search transaction, the descriptor is revalidated under it, and the vector and its completion commit in one bounded search transaction.
//! The transaction ends and its connection is released before the guard drops; nothing under the guard runs inference, decodes source, waits on the network, or sleeps.
//! Stale descriptors obsolete their jobs instead of completing them. Lost transaction replies reconcile from durable vector and job rows. Conflicting vectors and invalid shapes return errors without writing.

use std::time::Instant;

use host_runtime::synapse::inference::validate_unit_vector;
use kernel::{
    CurrentInputDescriptor, CurrentInputExpectation, CurrentInputGuard, EligibilityBinding,
    EligibilityVerdict, KernelError, KernelStore, StaleInput,
};
use retrieval::batch::VectorGeneration;
use retrieval::vectors::{
    CompletionOutcome, CompletionPhase, ObsoleteReason, Obsoletion, VectorCompletion,
    complete_embedding_observed, completion_status, obsolete_embedding,
};
use retrieval::{ProjectionError, read_identity};

use crate::search_catchup::{Refusal, classify};
use crate::search_projection::{
    SearchProjection, SearchProjectionError, StoreFailure, classify_store_failure,
};
use crate::search_writer::{Quarantine, QuarantineKind};

/// One vector ready to publish: the descriptor it was produced for, the generation it belongs to, and the accounting the inference charged.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorPublication<'a> {
    pub input: CurrentInputDescriptor,
    pub generation: &'a VectorGeneration,
    pub vector: &'a [f32],
    pub input_bytes: u64,
    pub input_tokens: u32,
}

/// A boundary emitted as publication progresses; branches emit only the events they cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationEvent {
    VectorValidated,
    GuardRequested,
    /// The kernel writer is held and the descriptor agreed with the expectation.
    GuardAcquired,
    /// The vector row is inserted and the search transaction is still open; the job is not yet complete.
    VectorStaged,
    /// The vector and completion statements have run and the search transaction is still open.
    LocalStaged,
    /// The search transaction has ended, or never began, and its connection is released.
    LocalReleased,
    GuardReleased,
    /// A stale current-input verdict has released the kernel writer.
    StaleWriterReleased,
    /// The local transaction's reply was lost and durable rows are being read after the kernel writer was released.
    Reconciling,
}

/// Why the job was made obsolete instead of completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObsoleteCause {
    /// The kernel's descriptor no longer matches the expectation.
    Canonical(StaleInput),
    /// Reconciliation found an obsolete job whose exact cause was not stored.
    ProjectedReconciled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publication {
    /// The vector and its completion are durable together.
    Embedded,
    /// The same vector was already durable.
    Replayed,
    /// No vector was written; the job is `obsolete`.
    Obsolete(ObsoleteCause),
}

#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    /// The vector fails the served-vector contract; dispatch for this job stops until an operator repairs the engine.
    #[error("the vector fails the served contract: {0}")]
    InvalidVector(String),
    /// A different vector is already durable for the same job; dispatch stops until an operator decides which is right.
    #[error("a different vector is already durable for the job")]
    IdempotencyConflict,
    /// The kernel writer stayed held past the deadline; the result lease is retained and the publication can be tried again.
    #[error("the kernel writer was not acquired before the deadline")]
    GuardDeadline,
    /// The eligibility binding's project does not name the input's scope. That verdict follows the binding rather than the input, so the job is left open and the result lease is retained.
    #[error("the eligibility binding's project does not name the input's scope")]
    WrongScope,
    /// The search connection or its write lock stayed held past the deadline; nothing was written and the result lease is retained.
    #[error("the search write lock was not acquired before the deadline")]
    SearchDeadline,
    /// The search transaction may or may not have committed and the durable rows do not show its effect; the result lease is retained.
    #[error("the local commit outcome is unresolved")]
    LocalCommitUnresolved,
    /// Another projection writer advanced the durable fence before this write began.
    #[error("the search projection writer was fenced before publication")]
    ProjectionFenced,
    /// The projection refused the completion before writing: no job, no occurrence, or another generation.
    #[error(transparent)]
    Refused(ProjectionError),
    #[error("the search projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

/// What the local transaction settled to before the kernel writer was released.
enum Settled {
    Published(Publication),
    /// The commit reply was lost; the durable rows decide once the guard is released.
    CompletionUnresolved,
    /// The obsoletion reply was lost; the durable job state decides once the stale writer is released.
    ObsoletionUnresolved(StaleInput),
    /// Quarantine synchronization waits until after the kernel writer is released.
    Quarantine {
        kind: QuarantineKind,
        detail: String,
    },
}

/// Publishes vectors for one projection against one kernel.
///
/// The caller must hold the daemon's process-lifetime instance fence while this publisher can run. The current-input guard excludes mutations through its `KernelStore`; the instance fence excludes a successor store that could otherwise advance the durable writer fence through a replaced lease namespace.
///
/// `publish` blocks on the kernel writer and the search connection, so it belongs on a blocking thread.
/// The observer runs on that thread. Callbacks before a release event must call neither the kernel nor the projection. `GuardReleased` and `StaleWriterReleased` are alternative branch events; their callbacks and `Reconciling` run after the kernel writer is released and may call either dependency. An early error may return without a release event, so observers must not wait for one without a bound.
pub struct EmbeddingPublisher<'a> {
    kernel: &'a KernelStore,
    projection: &'a SearchProjection,
    fault: Option<PublicationFault>,
}

/// Which reply the publication loses. Only the test-support entry point can set one; a production build never injects a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationFault {
    /// The transaction commits, then its reply arrives as a store failure.
    LoseLocalCommitReply,
    /// The transaction rolls back at COMMIT, and its reply arrives as a store failure.
    LoseLocalCommit,
}

impl<'a> EmbeddingPublisher<'a> {
    /// The daemon's process-lifetime instance fence must remain held for the returned publisher's lifetime.
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

    /// Validates the vector, takes the current-input guard by `deadline`, revalidates the descriptor for `eligibility`, and commits the vector with its completion.
    ///
    /// `now` is Unix-epoch milliseconds recorded on the vector and job rows.
    ///
    /// # Errors
    ///
    /// Every error path has released the guard and the search connection; none grants a later attempt any authorization.
    pub fn publish(
        &mut self,
        publication: &VectorPublication<'_>,
        eligibility: EligibilityBinding<'_>,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(PublicationEvent),
    ) -> Result<Publication, PublicationError> {
        self.fault = None;
        self.publish_inner(publication, eligibility, deadline, now, observer)
    }

    /// [`Self::publish`] under one injected fault.
    #[cfg(feature = "test-support")]
    pub fn publish_with_fault_for_test(
        &mut self,
        publication: &VectorPublication<'_>,
        eligibility: EligibilityBinding<'_>,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(PublicationEvent),
        fault: PublicationFault,
    ) -> Result<Publication, PublicationError> {
        self.fault = Some(fault);
        self.publish_inner(publication, eligibility, deadline, now, observer)
    }

    fn publish_inner(
        &mut self,
        publication: &VectorPublication<'_>,
        eligibility: EligibilityBinding<'_>,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(PublicationEvent),
    ) -> Result<Publication, PublicationError> {
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(PublicationError::Quarantined(quarantine));
        }
        validate_unit_vector(
            publication.generation.vector_dimension as usize,
            publication.vector,
        )
        .map_err(PublicationError::InvalidVector)?;
        observer(PublicationEvent::VectorValidated);

        observer(PublicationEvent::GuardRequested);
        let expectation = CurrentInputExpectation {
            object_id: publication.input.object_id.clone(),
            source_revision: publication.input.source_revision,
            occurrence_id: publication.input.detail.occurrence_id.clone(),
            payload_id: publication.input.detail.payload_id.clone(),
            artifact_digest: publication.input.detail.artifact_digest.clone(),
        };
        let (outcome, quarantine) =
            match self
                .kernel
                .guard_current_input(&expectation, eligibility, deadline)
            {
                Ok(Ok(guard)) => {
                    if guard.descriptor() != &publication.input {
                        return Err(PublicationError::Refused(ProjectionError::IdentityMismatch));
                    }
                    observer(PublicationEvent::GuardAcquired);
                    let outcome = self.commit(publication, &guard, deadline, now, observer);
                    let quarantine = match &outcome {
                        Ok(Settled::Quarantine { kind, detail }) => {
                            Some(self.projection.publish_quarantine_intent(*kind, detail))
                        }
                        _ => None,
                    };
                    drop(guard);
                    observer(PublicationEvent::GuardReleased);
                    (outcome, quarantine)
                }
                // Every other stale verdict is a fact about the input; this one is a fact about the binding.
                Ok(Err(stale))
                    if matches!(
                        stale.reason(),
                        StaleInput::Ineligible(EligibilityVerdict::WrongScope)
                    ) =>
                {
                    return Err(PublicationError::WrongScope);
                }
                Ok(Err(stale)) => {
                    let reason = stale.reason().clone();
                    let outcome = self.obsolete(
                        publication,
                        stale.database_incarnation_id(),
                        deadline,
                        now,
                        reason,
                    );
                    let quarantine = match &outcome {
                        Ok(Settled::Quarantine { kind, detail }) => {
                            Some(self.projection.publish_quarantine_intent(*kind, detail))
                        }
                        _ => None,
                    };
                    drop(stale);
                    observer(PublicationEvent::StaleWriterReleased);
                    (outcome, quarantine)
                }
                Err(KernelError::Deadline) => return Err(PublicationError::GuardDeadline),
                Err(error) => return Err(error.into()),
            };
        match outcome {
            Ok(Settled::Published(publication)) => Ok(publication),
            Ok(Settled::Quarantine { .. }) => Err(PublicationError::Quarantined(
                self.projection.synchronize_quarantine(
                    quarantine
                        .expect("quarantine outcome must publish intent before guard release"),
                ),
            )),
            // The store failed between BEGIN and COMMIT; the durable vector, not the error, says whether COMMIT took effect, and reading it needs no guard.
            Ok(Settled::CompletionUnresolved) => {
                observer(PublicationEvent::Reconciling);
                let status = self.projection.read(|conn| {
                    completion_status(
                        conn,
                        &publication.input.detail.occurrence_id,
                        &publication.generation.generation_id,
                    )
                });
                match status {
                    Ok(status) if status.has_durable_vector(publication.vector) => {
                        Ok(Publication::Embedded)
                    }
                    Ok(status) if status.job_state.as_deref() == Some("obsolete") => {
                        Ok(Publication::Obsolete(ObsoleteCause::ProjectedReconciled))
                    }
                    Ok(_) => Err(PublicationError::LocalCommitUnresolved),
                    Err(error) => Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
                }
            }
            Ok(Settled::ObsoletionUnresolved(stale)) => {
                observer(PublicationEvent::Reconciling);
                let status = self.projection.read(|conn| {
                    completion_status(
                        conn,
                        &publication.input.detail.occurrence_id,
                        &publication.generation.generation_id,
                    )
                });
                match status {
                    Ok(status) if status.job_state.as_deref() == Some("obsolete") => {
                        Ok(Publication::Obsolete(ObsoleteCause::Canonical(stale)))
                    }
                    Ok(_) => Err(PublicationError::LocalCommitUnresolved),
                    Err(error) => Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Runs the completion in one fenced search transaction and reports [`PublicationEvent::LocalReleased`] once it is over, whichever way it ended.
    ///
    /// The kernel writer is held throughout, so acquiring the search connection and its write lock is bounded by the same `deadline` that bounded the guard; a wait past it writes nothing.
    fn commit(
        &mut self,
        publication: &VectorPublication<'_>,
        guard: &CurrentInputGuard<'_>,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(PublicationEvent),
    ) -> Result<Settled, PublicationError> {
        let completion = VectorCompletion {
            input: guard.descriptor(),
            generation: publication.generation,
            vector: publication.vector,
            input_bytes: publication.input_bytes,
            input_tokens: publication.input_tokens,
        };
        let fault = self.fault;
        let mut applied = self.projection.write_within(deadline, |conn| {
            require_kernel_incarnation(conn, guard.database_incarnation_id())?;
            let outcome = complete_embedding_observed(conn, &completion, now, &mut |phase| {
                observer(match phase {
                    CompletionPhase::VectorInserted => PublicationEvent::VectorStaged,
                    CompletionPhase::JobEmbedded => PublicationEvent::LocalStaged,
                })
            })?;
            let published = match outcome {
                CompletionOutcome::Embedded => Publication::Embedded,
                CompletionOutcome::Replayed => Publication::Replayed,
                CompletionOutcome::Obsolete(ObsoleteReason::Tombstoned) => {
                    return Err(ProjectionError::CorruptRow);
                }
                CompletionOutcome::Obsolete(ObsoleteReason::PayloadChanged) => {
                    return Err(ProjectionError::CorruptRow);
                }
            };
            Self::inject_rollback(fault, published)
        });
        observer(PublicationEvent::LocalReleased);
        self.inject_lost_reply(&mut applied);
        match applied {
            Ok(published) => Ok(Settled::Published(published)),
            // The store returned before COMMIT, so nothing of the completion is durable.
            Err(SearchProjectionError::Projection(error)) => match classify(&error) {
                Refusal::OperatorRepair => match error {
                    ProjectionError::VectorConflict { .. } => {
                        Err(PublicationError::IdempotencyConflict)
                    }
                    ProjectionError::InvalidVector { reason } => {
                        Err(PublicationError::InvalidVector(reason.to_owned()))
                    }
                    error => Err(PublicationError::Refused(error)),
                },
                Refusal::Admission | Refusal::Identity => Err(PublicationError::Refused(error)),
                // A completion may reference an occurrence that was never queued;
                // classify it as refusal, not projection corruption.
                Refusal::Integrity => match error {
                    ProjectionError::UnknownOccurrence { .. } => {
                        Err(PublicationError::Refused(error))
                    }
                    error => Ok(Settled::Quarantine {
                        kind: QuarantineKind::Integrity,
                        detail: error.to_string(),
                    }),
                },
                Refusal::Storage => Ok(Settled::Quarantine {
                    kind: QuarantineKind::Storage,
                    detail: error.to_string(),
                }),
            },
            Err(SearchProjectionError::Quarantined(quarantine)) => {
                Err(PublicationError::Quarantined(quarantine))
            }
            Err(SearchProjectionError::Connection(error)) => Ok(Settled::Quarantine {
                kind: QuarantineKind::Integrity,
                detail: error,
            }),
            Err(SearchProjectionError::Store(error)) => match classify_store_failure(&error) {
                StoreFailure::Deadline => Err(PublicationError::SearchDeadline),
                StoreFailure::Rejected => Err(PublicationError::ProjectionFenced),
                StoreFailure::Integrity => Ok(Settled::Quarantine {
                    kind: QuarantineKind::Integrity,
                    detail: error.to_string(),
                }),
                StoreFailure::Unknown => Ok(Settled::CompletionUnresolved),
            },
        }
    }

    /// Records that the job's input is gone from the kernel while the stale verdict retains the kernel writer.
    ///
    /// The obsoletion is one keyed, state-predicated update, so a store failure whose effect is unknown is settled from the durable job state rather than by quarantine; only a refusal the store classifies as integrity or storage damage quarantines.
    fn obsolete(
        &mut self,
        publication: &VectorPublication<'_>,
        kernel_incarnation_id: &str,
        deadline: Instant,
        now: i64,
        stale: StaleInput,
    ) -> Result<Settled, PublicationError> {
        let occurrence_id = &publication.input.detail.occurrence_id;
        let generation_id = &publication.generation.generation_id;
        let fault = self.fault;
        let mut marked = self.projection.write_within(deadline, |conn| {
            require_kernel_incarnation(conn, kernel_incarnation_id)?;
            let outcome = obsolete_embedding(conn, &publication.input, generation_id, now)?;
            Self::inject_rollback(fault, outcome)
        });
        self.inject_lost_reply(&mut marked);
        match marked {
            Ok(Obsoletion::Marked | Obsoletion::AlreadyTerminal) => Ok(Settled::Published(
                Publication::Obsolete(ObsoleteCause::Canonical(stale)),
            )),
            Ok(Obsoletion::NoJob) => {
                Err(PublicationError::Refused(ProjectionError::NoPendingWork {
                    occurrence_id: occurrence_id.clone(),
                }))
            }
            Err(SearchProjectionError::Projection(error)) => match classify(&error) {
                Refusal::Integrity => Ok(Settled::Quarantine {
                    kind: QuarantineKind::Integrity,
                    detail: error.to_string(),
                }),
                Refusal::Storage => Ok(Settled::Quarantine {
                    kind: QuarantineKind::Storage,
                    detail: error.to_string(),
                }),
                Refusal::Admission | Refusal::Identity | Refusal::OperatorRepair => {
                    Err(PublicationError::Refused(error))
                }
            },
            Err(SearchProjectionError::Quarantined(quarantine)) => {
                Err(PublicationError::Quarantined(quarantine))
            }
            Err(SearchProjectionError::Connection(error)) => Ok(Settled::Quarantine {
                kind: QuarantineKind::Integrity,
                detail: error,
            }),
            Err(SearchProjectionError::Store(error)) => match classify_store_failure(&error) {
                StoreFailure::Deadline => Err(PublicationError::SearchDeadline),
                StoreFailure::Rejected => Err(PublicationError::ProjectionFenced),
                StoreFailure::Integrity => Ok(Settled::Quarantine {
                    kind: QuarantineKind::Integrity,
                    detail: error.to_string(),
                }),
                StoreFailure::Unknown => Ok(Settled::ObsoletionUnresolved(stale)),
            },
        }
    }

    fn enter_quarantine(
        &mut self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> PublicationError {
        PublicationError::Quarantined(self.projection.enter_quarantine(kind, error))
    }

    fn inject_lost_reply<T>(&self, outcome: &mut Result<T, SearchProjectionError>) {
        let lost_reply = match self.fault {
            Some(PublicationFault::LoseLocalCommitReply) => outcome.is_ok(),
            Some(PublicationFault::LoseLocalCommit) => matches!(
                outcome.as_ref(),
                Err(SearchProjectionError::Projection(ProjectionError::Sqlite(detail)))
                    if detail == ROLLBACK_FAULT_DETAIL
            ),
            None => false,
        };
        if lost_reply {
            *outcome = Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )));
        }
    }

    fn inject_rollback<T>(
        fault: Option<PublicationFault>,
        outcome: T,
    ) -> Result<T, ProjectionError> {
        if fault == Some(PublicationFault::LoseLocalCommit) {
            Err(ProjectionError::Sqlite(ROLLBACK_FAULT_DETAIL.to_owned()))
        } else {
            Ok(outcome)
        }
    }
}

const ROLLBACK_FAULT_DETAIL: &str = "fault: rolled back";

fn require_kernel_incarnation(
    conn: &storage::GuardedConn<'_>,
    kernel_incarnation_id: &str,
) -> Result<(), ProjectionError> {
    match read_identity(conn)? {
        Some(identity) if identity.kernel_incarnation_id == kernel_incarnation_id => Ok(()),
        _ => Err(ProjectionError::IdentityMismatch),
    }
}
