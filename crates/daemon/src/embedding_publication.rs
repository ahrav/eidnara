//! Publishes one validated vector into the search projection while the kernel's current-input guard excludes every canonical mutation of its source.
//!
//! The vector is validated before the guard is taken, the guard is taken without a search transaction, the descriptor is revalidated under it, and the vector and its completion commit in one bounded search transaction.
//! The transaction ends and its connection is released before the guard drops; nothing under the guard runs inference, decodes source, waits on the network, or sleeps.
//! A stale descriptor makes the job obsolete instead of completing it. An unknown local commit is reconciled from the durable vector. A conflicting vector or an invalid shape stops automatic dispatch for operator repair. Integrity and storage failures quarantine the projection for every writer.

use std::time::Instant;

use host_runtime::synapse::inference::validate_unit_vector;
use kernel::{
    CurrentInputExpectation, EligibilityBinding, EligibilityVerdict, KernelError, KernelStore,
    StaleInput,
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
    pub expectation: CurrentInputExpectation,
    pub generation: &'a VectorGeneration,
    pub vector: &'a [f32],
    pub input_bytes: u64,
    pub input_tokens: u32,
}

/// A boundary the publication crosses, in the order it crosses them.
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
    /// The completion's reply was lost and the durable rows are being read to settle it, after the guard dropped.
    Reconciling,
}

/// Why the job was made obsolete instead of completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObsoleteCause {
    /// The kernel's descriptor no longer matches the expectation.
    Canonical(StaleInput),
    /// The projection's own row disagrees with the vector's identity.
    Projected(ObsoleteReason),
    /// The projection reports an obsolete job without an exact cause.
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
    /// The projection refused the completion before writing: no job, no occurrence, or another generation.
    #[error(transparent)]
    Refused(ProjectionError),
    #[error("the search projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

/// What the guarded transaction settled to before the guard dropped.
enum Settled {
    Published(Publication),
    /// The commit reply was lost; the durable rows decide once the guard is released.
    Unresolved,
}

/// Publishes vectors for one projection against one kernel.
///
/// `publish` blocks on the kernel writer and the search connection and cannot be cancelled between `GuardAcquired` and `GuardReleased`, so it belongs on a blocking thread.
/// The observer runs on that thread while the guard, and at the staged events the search connection too, is held; it must call neither the kernel nor the projection.
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
        let guard =
            match self
                .kernel
                .guard_current_input(&publication.expectation, eligibility, deadline)
            {
                Ok(Ok(guard)) => guard,
                // Every other stale verdict is a fact about the input; this one is a fact about the binding.
                Ok(Err(StaleInput::Ineligible(EligibilityVerdict::WrongScope))) => {
                    return Err(PublicationError::WrongScope);
                }
                Ok(Err(stale)) => {
                    let kernel_incarnation_id = self.kernel.database_incarnation_id(deadline)?;
                    return self.obsolete(
                        publication,
                        &kernel_incarnation_id,
                        deadline,
                        now,
                        stale,
                    );
                }
                Err(KernelError::Deadline) => return Err(PublicationError::GuardDeadline),
                Err(error) => return Err(error.into()),
            };
        observer(PublicationEvent::GuardAcquired);
        let outcome = self.commit(
            publication,
            guard.database_incarnation_id(),
            deadline,
            now,
            observer,
        );
        drop(guard);
        observer(PublicationEvent::GuardReleased);
        match outcome {
            Ok(Settled::Published(publication)) => Ok(publication),
            // The store failed between BEGIN and COMMIT; the durable vector, not the error, says whether COMMIT took effect, and reading it needs no guard.
            Ok(Settled::Unresolved) => {
                observer(PublicationEvent::Reconciling);
                let status = self.projection.read(|conn| {
                    completion_status(
                        conn,
                        &publication.expectation.occurrence_id,
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
            Err(error) => Err(error),
        }
    }

    /// Runs the completion in one fenced search transaction and reports [`PublicationEvent::LocalReleased`] once it is over, whichever way it ended.
    ///
    /// The kernel writer is held throughout, so acquiring the search connection and its write lock is bounded by the same `deadline` that bounded the guard; a wait past it writes nothing.
    fn commit(
        &mut self,
        publication: &VectorPublication<'_>,
        kernel_incarnation_id: &str,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(PublicationEvent),
    ) -> Result<Settled, PublicationError> {
        let completion = VectorCompletion {
            occurrence_id: &publication.expectation.occurrence_id,
            generation: publication.generation,
            payload_id: &publication.expectation.payload_id,
            vector: publication.vector,
            input_bytes: publication.input_bytes,
            input_tokens: publication.input_tokens,
        };
        let roll_back = self.fault == Some(PublicationFault::LoseLocalCommit);
        let mut applied = self.projection.write_within(deadline, |conn| {
            require_kernel_incarnation(conn, kernel_incarnation_id)?;
            let outcome = complete_embedding_observed(conn, &completion, now, &mut |phase| {
                observer(match phase {
                    CompletionPhase::VectorInserted => PublicationEvent::VectorStaged,
                    CompletionPhase::JobEmbedded => PublicationEvent::LocalStaged,
                })
            })?;
            if roll_back {
                // A refusal from the closure rolls the transaction back; the reply is replaced below.
                return Err(ProjectionError::Sqlite("fault: rolled back".to_owned()));
            }
            Ok(outcome)
        });
        observer(PublicationEvent::LocalReleased);
        let lost_reply = match self.fault {
            Some(PublicationFault::LoseLocalCommitReply) => applied.is_ok(),
            Some(PublicationFault::LoseLocalCommit) => true,
            None => false,
        };
        if lost_reply {
            applied = Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )));
        }
        match applied {
            Ok(CompletionOutcome::Embedded) => Ok(Settled::Published(Publication::Embedded)),
            Ok(CompletionOutcome::Replayed) => Ok(Settled::Published(Publication::Replayed)),
            Ok(CompletionOutcome::Obsolete(reason)) => Ok(Settled::Published(
                Publication::Obsolete(ObsoleteCause::Projected(reason)),
            )),
            // The store returned before COMMIT, so nothing of the completion is durable.
            Err(SearchProjectionError::Projection(error)) => Err(match classify(&error) {
                Refusal::OperatorRepair => match error {
                    ProjectionError::VectorConflict { .. } => PublicationError::IdempotencyConflict,
                    ProjectionError::InvalidVector { reason } => {
                        PublicationError::InvalidVector(reason.to_owned())
                    }
                    error => PublicationError::Refused(error),
                },
                Refusal::Admission | Refusal::Identity => PublicationError::Refused(error),
                // A completion may reference an occurrence that was never queued;
                // classify it as refusal, not projection corruption.
                Refusal::Integrity => match error {
                    ProjectionError::UnknownOccurrence { .. } => PublicationError::Refused(error),
                    error => self.enter_quarantine(QuarantineKind::Integrity, &error),
                },
                Refusal::Storage => self.enter_quarantine(QuarantineKind::Storage, &error),
            }),
            Err(SearchProjectionError::Quarantined(quarantine)) => {
                Err(PublicationError::Quarantined(quarantine))
            }
            Err(SearchProjectionError::Connection(error)) => {
                Err(self.enter_quarantine(QuarantineKind::Integrity, &error))
            }
            Err(SearchProjectionError::Store(error)) => match classify_store_failure(&error) {
                StoreFailure::Deadline => Err(PublicationError::SearchDeadline),
                StoreFailure::Integrity => {
                    Err(self.enter_quarantine(QuarantineKind::Integrity, &error))
                }
                StoreFailure::Unknown => Ok(Settled::Unresolved),
            },
        }
    }

    /// Records that the job's input is gone from the kernel, outside any guard.
    ///
    /// The obsoletion is one keyed, state-predicated update, so a store failure whose effect is unknown is settled from the durable job state rather than by quarantine; only a refusal the store classifies as integrity or storage damage quarantines.
    fn obsolete(
        &mut self,
        publication: &VectorPublication<'_>,
        kernel_incarnation_id: &str,
        deadline: Instant,
        now: i64,
        stale: StaleInput,
    ) -> Result<Publication, PublicationError> {
        let occurrence_id = &publication.expectation.occurrence_id;
        let generation_id = &publication.generation.generation_id;
        let marked = self.projection.write_within(deadline, |conn| {
            require_kernel_incarnation(conn, kernel_incarnation_id)?;
            obsolete_embedding(
                conn,
                occurrence_id,
                generation_id,
                &publication.expectation.object_id,
                now,
            )
        });
        match marked {
            Ok(Obsoletion::Marked | Obsoletion::AlreadyTerminal) => {
                Ok(Publication::Obsolete(ObsoleteCause::Canonical(stale)))
            }
            Ok(Obsoletion::NoJob) => {
                Err(PublicationError::Refused(ProjectionError::NoPendingWork {
                    occurrence_id: occurrence_id.clone(),
                }))
            }
            Err(SearchProjectionError::Projection(error)) => Err(match classify(&error) {
                Refusal::Integrity => self.enter_quarantine(QuarantineKind::Integrity, &error),
                Refusal::Storage => self.enter_quarantine(QuarantineKind::Storage, &error),
                Refusal::Admission | Refusal::Identity | Refusal::OperatorRepair => {
                    PublicationError::Refused(error)
                }
            }),
            Err(SearchProjectionError::Quarantined(quarantine)) => {
                Err(PublicationError::Quarantined(quarantine))
            }
            Err(SearchProjectionError::Connection(error)) => {
                Err(self.enter_quarantine(QuarantineKind::Integrity, &error))
            }
            // The store failed between BEGIN and COMMIT; the durable job state, not the error, says whether the update took effect.
            Err(SearchProjectionError::Store(error))
                if classify_store_failure(&error) == StoreFailure::Unknown =>
            {
                let status = self
                    .projection
                    .read(|conn| completion_status(conn, occurrence_id, generation_id));
                match status {
                    Ok(status) if status.job_state.as_deref() == Some("obsolete") => {
                        Ok(Publication::Obsolete(ObsoleteCause::Canonical(stale)))
                    }
                    Ok(_) => Err(PublicationError::LocalCommitUnresolved),
                    Err(error) => Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
                }
            }
            Err(SearchProjectionError::Store(error)) => match classify_store_failure(&error) {
                StoreFailure::Deadline => Err(PublicationError::SearchDeadline),
                StoreFailure::Integrity => {
                    Err(self.enter_quarantine(QuarantineKind::Integrity, &error))
                }
                StoreFailure::Unknown => unreachable!("unknown store failures reconcile above"),
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
}

fn require_kernel_incarnation(
    conn: &storage::GuardedConn<'_>,
    kernel_incarnation_id: &str,
) -> Result<(), ProjectionError> {
    match read_identity(conn)? {
        Some(identity) if identity.kernel_incarnation_id == kernel_incarnation_id => Ok(()),
        _ => Err(ProjectionError::IdentityMismatch),
    }
}
