//! Drives durable pending embedding work through the in-process Synapse job table to guarded completion.
//!
//! One pass binds the projection to the verified lane, then for each eligible job runs exact preflight on the stored input, admits its stable identity to the job table, charges one attempt, polls the result lease, and publishes the vector under the kernel's current-input guard.
//! The projection rows are the only queue: a job identifier is the SHA-256 of its occurrence and generation, and the job-table key is the canonical key of that identifier and text, so duplicate admission and duplicate polling name the same work.
//! A host restart makes every admitted job unreachable; rebinding returns them to pending with their attempts kept. Every non-success records a disposition on the row before the pass moves on; a disposition that cannot be recorded quarantines the dispatcher, because uncertain accounting must stop dispatch.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use host_runtime::synapse::{
    DenseUnavailable, EmbeddingInputLimits, InferenceFailureKind, LaneInfo, LaneUnavailableState,
    PollOutcome, SubmitOutcome, SynapseComponent, SynapseStatus, failure_is_permanent,
};
use kernel::{CurrentInputExpectation, EligibilityBinding, KernelError, KernelStore};
use retrieval::ProjectionError;
use retrieval::dispatch::{
    Admission, BindingOutcome, DispatchJob, Disposition, EXHAUSTED, EpisodeGrant, LaneBinding,
    bind_lane, charge_admission, eligible_jobs, job_ledger, rebind_host_job, record_retry,
    stop_job,
};
use retrieval::vectors::obsolete_embedding;

use crate::embedding_publication::{
    EmbeddingPublisher, Publication, PublicationError, VectorPublication,
};
use crate::search_catchup::{Refusal, classify};
use crate::search_projection::{SearchProjection, SearchProjectionError};
use crate::search_writer::{Quarantine, QuarantineKind};

/// Finite bounds one pass runs under. `retry_after` and `grant.deadline` are in the caller's `now` units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchBounds {
    pub max_jobs: NonZeroUsize,
    pub grant: EpisodeGrant,
    pub retry_after: i64,
    pub result_wait: Duration,
    pub guard_deadline: Duration,
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, PartialEq)]
pub enum DispatchEvent {
    Bound(BindingOutcome),
    /// The host holds the job and the row's episode carries `attempts` charged so far; re-admission after a job-table eviction reports the same count.
    Admitted {
        job_id: String,
        host_job_id: String,
        attempts: u32,
    },
    Published {
        job_id: String,
        outcome: Publication,
    },
    Retried {
        job_id: String,
        kind: &'static str,
    },
    Stopped {
        job_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    LaneUnavailable(LaneUnavailableState),
    ProjectionIdentity,
    /// The projection was built for a different product than the lane serves.
    BindingMismatch,
    IdentityChanged,
    HostClosing,
    /// The kernel writer stayed busy past the guard deadline; the row keeps its admitted state and result so the next pass publishes without a new charge.
    GuardDeadline,
    /// The completion's outcome is unknown; the row keeps its admitted state so the next pass reconciles it from the durable vector instead of charging again.
    LocalCommitUnresolved,
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("embedding dispatch is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    /// The store refused the lane binding or eligible-row read before any disposition, so the pass can run again.
    #[error(transparent)]
    Retryable(SearchProjectionError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchFault {
    /// The lane binding's store call fails before it runs.
    RefuseBinding,
    /// The admission charge's statement fails, so its transaction rolls back and the row stays pending.
    RefuseChargeStatement,
    /// The admission charge commits, then its reply arrives as a store failure.
    LoseChargeReply,
}

/// `run_pass` blocks and requires a multi-threaded Tokio runtime; `block_in_place` yields the worker while the pass waits.
pub struct EmbeddingDispatcher<'a> {
    kernel: &'a KernelStore,
    projection: &'a SearchProjection,
    synapse: &'a SynapseComponent,
    quarantine: Option<Quarantine>,
    fault: Option<DispatchFault>,
}

impl<'a> EmbeddingDispatcher<'a> {
    pub fn new(
        kernel: &'a KernelStore,
        projection: &'a SearchProjection,
        synapse: &'a SynapseComponent,
    ) -> Self {
        Self {
            kernel,
            projection,
            synapse,
            quarantine: None,
            fault: None,
        }
    }

    /// Arms `fault` for the next store call it names; the fault is consumed when it fires.
    #[cfg(feature = "test-support")]
    pub fn inject_fault_for_test(&mut self, fault: DispatchFault) {
        self.fault = Some(fault);
    }

    fn take_fault(&mut self, fault: DispatchFault) -> bool {
        if self.fault == Some(fault) {
            self.fault = None;
            true
        } else {
            false
        }
    }

    /// Runs one pass and reports what stopped it early, if anything; every job's disposition reaches `observer`.
    ///
    /// The pass sleeps while it polls, so it must not keep a runtime worker: on a worker it hands the worker's tasks off first, on a blocking thread it runs directly, and on a `current_thread` runtime it panics rather than starve the inference it waits for.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError::Retryable`] when the store refuses the lane binding or the eligible-row read, [`DispatchError::Quarantined`] once any disposition write fails or is refused and on every later call of this dispatcher, and [`DispatchError::Kernel`] when the kernel guard fails for a reason other than its deadline.
    pub fn run_pass(
        &mut self,
        eligibility: EligibilityBinding<'_>,
        bounds: &DispatchBounds,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        tokio::task::block_in_place(|| self.pass(eligibility, bounds, now, observer))
    }

    fn pass(
        &mut self,
        eligibility: EligibilityBinding<'_>,
        bounds: &DispatchBounds,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        if let Some(quarantine) = &self.quarantine {
            return Err(DispatchError::Quarantined(quarantine.clone()));
        }
        let lane = match serving(self.synapse.status()) {
            Ok(lane) => lane,
            Err(state) => return Ok(Some(Blocked::LaneUnavailable(state))),
        };
        let binding = lane_binding(&lane, self.synapse.host_incarnation());
        let bound = if self.take_fault(DispatchFault::RefuseBinding) {
            Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )))
        } else {
            self.projection.write(|conn| bind_lane(conn, &binding, now))
        };
        match self.before_dispositions(bound)? {
            BindingOutcome::Mismatch => return Ok(Some(Blocked::BindingMismatch)),
            BindingOutcome::Unbuilt => return Ok(Some(Blocked::ProjectionIdentity)),
            outcome => observer(DispatchEvent::Bound(outcome)),
        }
        let jobs = self
            .projection
            .read(|conn| eligible_jobs(conn, bounds.max_jobs, now));
        let jobs = self.before_dispositions(jobs)?;
        let pass = Pass {
            lane: &lane,
            binding: &binding,
            eligibility,
            bounds,
            now,
        };
        for job in &jobs {
            if let Some(blocked) = self.drive(job, &pass, observer)? {
                return Ok(Some(blocked));
            }
        }
        Ok(None)
    }

    /// Store failures before any disposition are retryable because nothing about the ledger is uncertain yet.
    fn before_dispositions<T>(
        &mut self,
        result: Result<T, SearchProjectionError>,
    ) -> Result<T, DispatchError> {
        result.map_err(|error| match &error {
            SearchProjectionError::Projection(refusal)
                if !matches!(classify(refusal), Refusal::Storage) =>
            {
                self.enter_quarantine(QuarantineKind::Integrity, &error)
            }
            _ => DispatchError::Retryable(error),
        })
    }

    /// Takes one job as far as this pass can: through admission and charging for a pending row, then through the result poll and publication.
    fn drive(
        &mut self,
        job: &DispatchJob,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        // A generation the lane does not serve is a model mismatch: the work is obsolete, not failed.
        if !pass.binding.serves(&job.generation) {
            self.write(|conn| {
                obsolete_embedding(
                    conn,
                    &job.occurrence_id,
                    &job.generation.generation_id,
                    pass.now,
                )
            })?;
            observer(DispatchEvent::Stopped {
                job_id: job.job_id.clone(),
                reason: "generation_mismatch".to_owned(),
            });
            return Ok(None);
        }
        let mut host_job_id = match &job.host_job_id {
            Some(host_job_id) if job.state == "admitted" => {
                if let Some(reason) = job.completion_refusal(pass.bounds.grant, pass.now) {
                    return self.stop(job, reason, pass.now, observer);
                }
                host_job_id.clone()
            }
            _ => {
                // Refuse the episode before host admission to avoid inference for refused work.
                if let Some(reason) = job.episode_refusal(pass.bounds.grant, pass.now) {
                    return self.stop(job, reason, pass.now, observer);
                }
                match self.admit(job, pass, observer)? {
                    Ok(host_job_id) => host_job_id,
                    Err(blocked) => return Ok(blocked),
                }
            }
        };
        let item_id = job.item_id();
        let mut readmitted = false;
        let mut started = Instant::now();
        loop {
            match self
                .synapse
                .poll_admitted(&host_job_id, &item_id, &job.text)
            {
                PollOutcome::Pending { .. } if started.elapsed() < pass.bounds.result_wait => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                // The host still holds the job; the row stays admitted for the next pass to poll.
                PollOutcome::Pending { .. } => return Ok(None),
                PollOutcome::Page(page) => {
                    let Some((_, _, vector)) =
                        page.vectors.iter().find(|(id, _, _)| id == &item_id)
                    else {
                        return self.stop(job, "malformed_request", pass.now, observer);
                    };
                    // The stored input is judged again before its result is trusted: the exact count is what the completion is charged, and a lane that no longer admits the input cannot complete it.
                    let admitted = match self
                        .synapse
                        .preflight_embedding(EmbeddingInputLimits::of_lane(pass.lane), &job.text)
                    {
                        Ok(admitted) => admitted,
                        Err(refusal) => {
                            return self.dense_unavailable(job, refusal, pass, observer);
                        }
                    };
                    // `page` stays alive through publication so its lease keeps the result bytes counted while the vector is in use.
                    let publication = VectorPublication {
                        expectation: CurrentInputExpectation {
                            object_id: job.source_object_id.clone(),
                            source_revision: job.revision,
                            occurrence_id: job.occurrence_id.clone(),
                            payload_id: job.payload_id.clone(),
                            artifact_digest: job.source_artifact_digest.clone(),
                        },
                        generation: &job.generation,
                        vector,
                        input_bytes: job.text.len() as u64,
                        input_tokens: admitted.tokens().get(),
                    };
                    return self.publish(job, &publication, pass, observer);
                }
                PollOutcome::Failed { code, .. } if !failure_is_permanent(&code) => {
                    return self.retry(job, "execution_failure", pass, observer);
                }
                // A worker that found the lane down fails its job with the lane's reason; that is the lane's disposition, not the input's, so the row keeps its admitted state until a serving host reconciles it.
                PollOutcome::Failed { code, .. } => {
                    return match serving(self.synapse.status()) {
                        Ok(_) => self.stop(job, &code, pass.now, observer),
                        Err(state) => Ok(Some(Blocked::LaneUnavailable(state))),
                    };
                }
                // One re-admission per pass keeps the pass finite.
                PollOutcome::Restarted if readmitted => return Ok(None),
                PollOutcome::Restarted => match self.readmit(job, &host_job_id, pass, observer)? {
                    Ok(rebound) => {
                        host_job_id = rebound;
                        readmitted = true;
                        started = Instant::now();
                    }
                    Err(blocked) => return Ok(blocked),
                },
                PollOutcome::KeyMismatch | PollOutcome::BadCursor => {
                    return self.stop(job, "malformed_request", pass.now, observer);
                }
            }
        }
    }

    fn submit(
        &mut self,
        job: &DispatchJob,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Result<String, Option<Blocked>>, DispatchError> {
        let admitted = match self
            .synapse
            .preflight_embedding(EmbeddingInputLimits::of_lane(pass.lane), &job.text)
        {
            Ok(admitted) => admitted,
            Err(refusal) => {
                return self
                    .dense_unavailable(job, refusal, pass, observer)
                    .map(Err);
            }
        };
        // The host item is the episode, so a new episode never reuses a job the table retains from a stopped one.
        Ok(
            match self.synapse.submit_admitted(&admitted, &job.item_id()) {
                Ok(SubmitOutcome::Queued { job_id }) => Ok(job_id),
                Ok(SubmitOutcome::Full) => {
                    self.retry(job, "admission_full", pass, observer).map(Err)?
                }
                Ok(SubmitOutcome::Refused(reason)) => {
                    self.stop(job, reason, pass.now, observer).map(Err)?
                }
                Ok(SubmitOutcome::Closing) => Err(Some(Blocked::HostClosing)),
                Err(refusal) => self
                    .dense_unavailable(job, refusal, pass, observer)
                    .map(Err)?,
            },
        )
    }

    fn admit(
        &mut self,
        job: &DispatchJob,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Result<String, Option<Blocked>>, DispatchError> {
        let host_job_id = match self.submit(job, pass, observer)? {
            Ok(host_job_id) => host_job_id,
            Err(blocked) => return Ok(Err(blocked)),
        };
        let charged = if self.take_fault(DispatchFault::RefuseChargeStatement) {
            Err(SearchProjectionError::Projection(ProjectionError::Sqlite(
                "database is locked".to_owned(),
            )))
        } else {
            let charged = self.projection.write(|conn| {
                charge_admission(
                    conn,
                    &job.job_id,
                    pass.binding,
                    &host_job_id,
                    pass.bounds.grant,
                    pass.now,
                )
            });
            if self.take_fault(DispatchFault::LoseChargeReply) && charged.is_ok() {
                Err(SearchProjectionError::Store(storage::StoreError::Backend(
                    "database is locked".to_owned(),
                )))
            } else {
                charged
            }
        };
        let charged = match charged {
            Ok(charged) => charged,
            Err(SearchProjectionError::Projection(error))
                if !matches!(classify(&error), Refusal::Storage) =>
            {
                return Err(self.enter_quarantine(QuarantineKind::Integrity, &error));
            }
            // The charge's statement failed or its reply was lost: the row, not the error, says whether it committed. A row still pending was not charged and is re-admitted by the next pass; anything else is unknown and stops dispatch.
            Err(lost) => match self.projection.read(|conn| job_ledger(conn, &job.job_id)) {
                Ok(Some(ledger))
                    if ledger.state == "admitted"
                        && ledger.host_job_id.as_deref() == Some(host_job_id.as_str()) =>
                {
                    Admission::AlreadyCharged
                }
                Ok(Some(ledger)) if ledger.state == "pending" => return Ok(Err(None)),
                _ => return Err(self.enter_quarantine(QuarantineKind::Storage, &lost)),
            },
        };
        let attempts = match charged {
            Admission::Charged { attempts } => attempts,
            Admission::AlreadyCharged => job.attempts + 1,
            Admission::Stopped(reason) => {
                observer(DispatchEvent::Stopped {
                    job_id: job.job_id.clone(),
                    reason: reason.to_owned(),
                });
                return Ok(Err(None));
            }
            Admission::NotPending => return Ok(Err(None)),
        };
        observer(DispatchEvent::Admitted {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.clone(),
            attempts,
        });
        Ok(Ok(host_job_id))
    }

    fn readmit(
        &mut self,
        job: &DispatchJob,
        evicted: &str,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Result<String, Option<Blocked>>, DispatchError> {
        let host_job_id = match self.submit(job, pass, observer)? {
            Ok(host_job_id) => host_job_id,
            Err(blocked) => return Ok(Err(blocked)),
        };
        let rebound =
            self.write(|conn| rebind_host_job(conn, &job.job_id, evicted, &host_job_id, pass.now))?;
        if !rebound {
            return Ok(Err(None));
        }
        observer(DispatchEvent::Admitted {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.clone(),
            attempts: job.attempts,
        });
        Ok(Ok(host_job_id))
    }

    fn publish(
        &mut self,
        job: &DispatchJob,
        publication: &VectorPublication<'_>,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        let mut publisher = EmbeddingPublisher::new(self.kernel, self.projection);
        let deadline = Instant::now() + pass.bounds.guard_deadline;
        match publisher.publish(
            publication,
            pass.eligibility,
            deadline,
            pass.now,
            &mut |_| {},
        ) {
            Ok(outcome) => {
                observer(DispatchEvent::Published {
                    job_id: job.job_id.clone(),
                    outcome,
                });
                Ok(None)
            }
            Err(PublicationError::InvalidVector(_)) => {
                self.stop(job, "invalid_vector", pass.now, observer)
            }
            Err(PublicationError::IdempotencyConflict) => {
                self.stop(job, "idempotency_conflict", pass.now, observer)
            }
            Err(PublicationError::GuardDeadline) => Ok(Some(Blocked::GuardDeadline)),
            Err(PublicationError::LocalCommitUnresolved) => {
                Ok(Some(Blocked::LocalCommitUnresolved))
            }
            // The publisher only hands back admission and identity refusals; its other classes already quarantined it.
            Err(PublicationError::Refused(error)) => match classify(&error) {
                Refusal::Identity => Ok(Some(Blocked::IdentityChanged)),
                // The row closed under another writer; the ledger already says so.
                _ => Ok(None),
            },
            Err(PublicationError::Quarantined(quarantine)) => {
                self.quarantine = Some(quarantine.clone());
                Err(DispatchError::Quarantined(quarantine))
            }
            Err(PublicationError::Kernel(error)) => Err(error.into()),
        }
    }

    /// Maps a preflight or submission refusal to its disposition: lane states block the pass, input refusals stop the row with a non-content reason, and a busy lane or execution failure is retried under the same episode.
    fn dense_unavailable(
        &mut self,
        job: &DispatchJob,
        refusal: DenseUnavailable,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        match refusal {
            DenseUnavailable::LaneUnavailable { state } => {
                Ok(Some(Blocked::LaneUnavailable(state)))
            }
            DenseUnavailable::IdentityChanged => Ok(Some(Blocked::IdentityChanged)),
            DenseUnavailable::LaneBusy { .. }
            | DenseUnavailable::Inference(InferenceFailureKind::Execution)
            | DenseUnavailable::CountUnavailable(InferenceFailureKind::Execution) => {
                self.retry(job, "execution_failure", pass, observer)
            }
            DenseUnavailable::Inference(_) | DenseUnavailable::CountUnavailable(_) => {
                self.stop(job, "artifact_invalid", pass.now, observer)
            }
            DenseUnavailable::EmptyInput | DenseUnavailable::ZeroTokens { .. } => {
                self.stop(job, "input_empty", pass.now, observer)
            }
            DenseUnavailable::ByteOverflow { .. } | DenseUnavailable::TokenOverflow { .. } => {
                self.stop(job, "input_over_limit", pass.now, observer)
            }
        }
    }

    fn retry(
        &mut self,
        job: &DispatchJob,
        kind: &'static str,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        let retry_at = pass.now + pass.bounds.retry_after;
        let job_id = job.job_id.clone();
        match self.write(|conn| record_retry(conn, &job.job_id, kind, retry_at, pass.now))? {
            Disposition::Retry => observer(DispatchEvent::Retried { job_id, kind }),
            Disposition::Exhausted => observer(DispatchEvent::Stopped {
                job_id,
                reason: EXHAUSTED.to_owned(),
            }),
            Disposition::NotOpen => {}
        }
        Ok(None)
    }

    fn stop(
        &mut self,
        job: &DispatchJob,
        reason: &str,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        if self.write(|conn| stop_job(conn, &job.job_id, reason, now))? {
            observer(DispatchEvent::Stopped {
                job_id: job.job_id.clone(),
                reason: reason.to_owned(),
            });
        }
        Ok(None)
    }

    /// Every disposition write that fails quarantines: a refusal means the row's state no longer describes the work, and a store failure leaves it unknown whether the disposition committed.
    fn write<T>(
        &mut self,
        f: impl FnOnce(&storage::GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<T, DispatchError> {
        self.projection.write(f).map_err(|error| match &error {
            SearchProjectionError::Projection(refusal)
                if !matches!(classify(refusal), Refusal::Storage) =>
            {
                self.enter_quarantine(QuarantineKind::Integrity, &error)
            }
            _ => self.enter_quarantine(QuarantineKind::Storage, &error),
        })
    }

    fn enter_quarantine(
        &mut self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> DispatchError {
        let quarantine = Quarantine::new(kind, error);
        self.quarantine = Some(quarantine.clone());
        DispatchError::Quarantined(quarantine)
    }
}

/// What one pass holds fixed for every job it drives.
struct Pass<'a> {
    lane: &'a LaneInfo,
    binding: &'a LaneBinding,
    eligibility: EligibilityBinding<'a>,
    bounds: &'a DispatchBounds,
    now: i64,
}

fn serving(status: SynapseStatus) -> Result<LaneInfo, LaneUnavailableState> {
    match status {
        SynapseStatus::Ready(lane) => Ok(lane),
        SynapseStatus::Starting => Err(LaneUnavailableState::Starting),
        SynapseStatus::Disabled { .. } => Err(LaneUnavailableState::Disabled),
        SynapseStatus::Failing { .. } => Err(LaneUnavailableState::Failing),
    }
}

/// The binding a lane implies: the lane fingerprint is the verified bundle fingerprint the projection identity records as its tokenizer fingerprint.
pub fn lane_binding(lane: &LaneInfo, host_incarnation: &str) -> LaneBinding {
    LaneBinding {
        embedding_model: lane.model.clone(),
        bundle_fingerprint: lane.fingerprint.clone(),
        vector_dimension: u32::try_from(lane.dims).unwrap_or(u32::MAX),
        table_epoch: lane.table_epoch,
        host_incarnation: host_incarnation.to_owned(),
    }
}
