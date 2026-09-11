//! Publishes one validated vector into the search projection while the kernel's current-input guard excludes every canonical mutation of its source.
//!
//! The vector is validated before the guard is taken, the guard is taken without a search transaction, the descriptor is revalidated under it, and the vector and its completion commit in one bounded search transaction.
//! The transaction ends and its connection is released before the guard drops; nothing under the guard runs inference, decodes source, waits on the network, or sleeps.
//! A stale descriptor makes the job obsolete instead of completing it. An unknown local commit is reconciled from the durable vector. A conflicting vector or an invalid shape stops automatic dispatch for operator repair. Integrity and storage failures quarantine the publisher.

use std::time::Instant;

use host_runtime::synapse::inference::validate_unit_vector;
use kernel::{CurrentInputExpectation, EligibilityBinding, KernelError, KernelStore, StaleInput};
use retrieval::ProjectionError;
use retrieval::batch::VectorGeneration;
use retrieval::vectors::{
    CompletionOutcome, CompletionPhase, ObsoleteReason, Obsoletion, VectorCompletion,
    complete_embedding_observed, completion_status, obsolete_embedding,
};

use crate::search_catchup::{Refusal, classify};
use crate::search_projection::{SearchProjection, SearchProjectionError};
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
    /// The search transaction may or may not have committed and the durable rows show no vector; the result lease is retained.
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
    quarantine: Option<Quarantine>,
    fault: Option<PublicationFault>,
}

/// Which reply the publication loses. Only the test-support entry point can set one; a production build never injects a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationFault {
    /// The transaction commits, then its reply arrives as a store failure.
    LoseLocalCommitReply,
}

impl<'a> EmbeddingPublisher<'a> {
    pub fn new(kernel: &'a KernelStore, projection: &'a SearchProjection) -> Self {
        Self {
            kernel,
            projection,
            quarantine: None,
            fault: None,
        }
    }

    pub fn quarantine(&self) -> Option<&Quarantine> {
        self.quarantine.as_ref()
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
        if let Some(quarantine) = &self.quarantine {
            return Err(PublicationError::Quarantined(quarantine.clone()));
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
                Ok(Err(stale)) => return self.obsolete(publication, now, stale),
                Err(KernelError::Deadline) => return Err(PublicationError::GuardDeadline),
                Err(error) => return Err(error.into()),
            };
        observer(PublicationEvent::GuardAcquired);
        let outcome = self.commit(publication, now, observer);
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
                    Ok(_) => Err(PublicationError::LocalCommitUnresolved),
                    Err(error) => Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Runs the completion in one fenced search transaction and reports [`PublicationEvent::LocalReleased`] once it is over, whichever way it ended.
    fn commit(
        &mut self,
        publication: &VectorPublication<'_>,
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
        let mut applied = self.projection.write(|conn| {
            complete_embedding_observed(conn, &completion, now, &mut |phase| {
                observer(match phase {
                    CompletionPhase::VectorInserted => PublicationEvent::VectorStaged,
                    CompletionPhase::JobEmbedded => PublicationEvent::LocalStaged,
                })
            })
        });
        observer(PublicationEvent::LocalReleased);
        if self.fault == Some(PublicationFault::LoseLocalCommitReply) && applied.is_ok() {
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
                Refusal::Integrity => self.enter_quarantine(QuarantineKind::Integrity, &error),
                Refusal::Storage => self.enter_quarantine(QuarantineKind::Storage, &error),
            }),
            Err(_) => Ok(Settled::Unresolved),
        }
    }

    /// Records that the job's input is gone from the kernel, outside any guard.
    fn obsolete(
        &mut self,
        publication: &VectorPublication<'_>,
        now: i64,
        stale: StaleInput,
    ) -> Result<Publication, PublicationError> {
        let marked = self.projection.write(|conn| {
            obsolete_embedding(
                conn,
                &publication.expectation.occurrence_id,
                &publication.generation.generation_id,
                now,
            )
        });
        match marked {
            Ok(Obsoletion::Marked | Obsoletion::AlreadyTerminal) => {
                Ok(Publication::Obsolete(ObsoleteCause::Canonical(stale)))
            }
            Ok(Obsoletion::NoJob) => {
                Err(PublicationError::Refused(ProjectionError::NoPendingWork {
                    occurrence_id: publication.expectation.occurrence_id.clone(),
                }))
            }
            Err(error) => Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
        }
    }

    fn enter_quarantine(
        &mut self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> PublicationError {
        let quarantine = Quarantine::new(kind, error);
        self.quarantine = Some(quarantine.clone());
        PublicationError::Quarantined(quarantine)
    }
}
