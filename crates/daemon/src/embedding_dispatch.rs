//! Drives durable pending embedding work through the in-process Synapse job table to guarded completion.
//!
//! Pass order is lane binding, eligible-job selection, admission, result polling, and guarded publication.
//! `admit` passes one `item_id` to `submit` and `charge_admission`; `EpisodeChanged` defers the row.
//! The projection rows are the only queue. The job-table key binds the episode and text to the frozen lane identity, so duplicate admission and polling within that lane name the same work.
//! `bind_lane` returns rows owned by another host incarnation to pending and leaves their attempts unchanged.
//! Disposition failures enter `SearchProjection`'s shared quarantine, checked before host submission, terminal obsoletion, and disposition writes.
//! One `EvalBudget` bounds a pass: its absolute deadline is the kernel guard's deadline and caps every result wait, and its sticky cancellation stops the pass at the next job, the next poll, or the moment before native work would start. A started local transaction runs to its bounded end, and its wait for the projection file is bounded by the store's own busy timeout rather than the budget; a started native call is never preempted, so a cancelled pass leaves its admitted row for a later pass to poll.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use host_runtime::synapse::{
    DenseUnavailable, InferenceFailureKind, LaneInfo, LaneUnavailableState, PollOutcome,
    SubmitOutcome, SynapseComponent, SynapseStatus, failure_is_permanent,
};
use kernel::applicability::EvalBudget;
use kernel::{
    CurrentInputDescriptor, CurrentInputExpectation, EligibilityBinding, EligibilityCandidate,
    EligibilityVerdict, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES, StaleInput,
};
use retrieval::ProjectionError;
use retrieval::dispatch::{
    Admission, BindingOutcome, CandidateReadiness, DispatchCandidate, DispatchCursor, DispatchJob,
    Disposition, EXHAUSTED, EpisodeGrant, JobLedger, LaneBinding, bind_lane, charge_admission,
    dispatch_job, job_ledger, obsolete_judged_job, open_job_candidates, rebind_host_job,
    record_retry, stop_job,
};
use retrieval::vectors::{Obsoletion, completion_status};

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
    /// `deadline` uses the caller-defined logical units supplied as `run_pass(now)`.
    pub grant: EpisodeGrant,
    /// Added to the pass's frozen logical `now`; it is not a wall-clock duration.
    pub retry_after: i64,
    /// How long one job's result is awaited before the pass moves on to the next job; monotonic, independent of the logical episode clock, and only ever shortened by the budget's deadline.
    pub result_wait: Duration,
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_ELIGIBILITY_PAGES_PER_PASS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum DispatchEvent {
    Bound(BindingOutcome),
    /// A job entered a stage; `deadline` is the budget's own, so an adapter that renewed it would show a different instant here.
    Stage {
        job_id: String,
        stage: Stage,
        deadline: Instant,
    },
    /// The host accepted the job; native work may run from this point, whatever the charge that follows decides.
    Submitted {
        job_id: String,
        host_job_id: String,
    },
    /// The charge did not bind the row to this host job: it rolled back, or the row had changed underneath it. The host still runs the job, but no row will ask for its result.
    Orphaned {
        job_id: String,
        host_job_id: String,
    },
    /// The host holds the job and the row's episode carries `attempts` charged so far; re-admission after a job-table eviction reports the same count.
    Admitted {
        job_id: String,
        host_job_id: String,
        attempts: u32,
    },
    /// The vector is durable and the row is settled; `host_job_id` names the host job whose result it was, so a census can retire exactly that job.
    Published {
        job_id: String,
        host_job_id: String,
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

/// The stages of one job's path under a pass, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Admit,
    Poll,
    Publish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// The pass's budget was cancelled or its deadline passed; unfinished rows keep their state.
    BudgetExhausted,
    /// The budget carries no deadline, so the kernel guard has no bound; a pass needs an absolute deadline.
    UnboundedBudget,
    LaneUnavailable(LaneUnavailableState),
    ProjectionIdentity,
    /// The projection was built for a different product than the lane serves.
    BindingMismatch,
    IdentityChanged,
    HostClosing,
    /// The kernel writer stayed busy past the guard deadline; the row keeps its admitted state and result so the next pass publishes without a new charge.
    GuardDeadline,
    /// The eligibility binding names another project; the admitted row is unchanged so publication can resume under the right scope without a new charge.
    WrongScope,
    /// The search write lock stayed busy past the deadline; the admitted row is unchanged so publication can resume without a new charge.
    SearchDeadline,
    /// The completion's outcome is unknown; the row keeps its admitted state so the next pass reconciles it from the durable vector instead of charging again.
    LocalCommitUnresolved,
    /// The pass's logical `now` is outside the Unix-epoch-millisecond domain, so no publication can settle under it; the admitted row is unchanged until a pass with a valid clock runs.
    PassTimeInvalid,
    /// Another projection writer advanced the durable fence before publication began; nothing was written and only a rebuilt writer can dispatch again.
    ProjectionFenced,
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("the search projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    /// The store refused the lane binding or eligible-row read before any disposition, so the pass can run again.
    #[error(transparent)]
    Retryable(SearchProjectionError),
    #[error("kernel eligibility read is retryable: {0}")]
    RetryableKernel(KernelError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchFault {
    /// The lane binding's store call fails before it runs.
    RefuseBinding,
    /// The eligible-row read fails before it runs, after the lane is bound.
    RefuseEligibilityRead,
    /// The admission charge's statement fails, so its transaction rolls back and the row stays pending.
    RefuseChargeStatement,
    /// The admission charge commits, then its reply arrives as a store failure.
    LoseChargeReply,
    /// The ledger read that reconciles a lost charge reply fails, so the charge's outcome stays unknown.
    RefuseLedgerRead,
    /// Injects a store failure after a successful obsoletion write.
    LoseObsoletionReply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LostChargeResolution {
    AlreadyCharged,
    Retry,
    Defer,
}

enum SubmitFailure {
    Unavailable(DenseUnavailable),
    Quarantined(Quarantine),
    /// The budget ended between preflight and host admission, before any native work began.
    BudgetExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EligibilityCursorBinding {
    project: kernel::ProjectScope,
    destination: kernel::ArtifactDestination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanPosition {
    binding: EligibilityCursorBinding,
    cursor: Option<DispatchCursor>,
    revisit_at: Option<i64>,
}

fn lost_charge_resolution(
    original_attempts: u32,
    host_job_id: &str,
    expected_episode_id: &str,
    ledger: &JobLedger,
) -> Option<LostChargeResolution> {
    if ledger.state == "admitted"
        && ledger.host_job_id.as_deref() == Some(host_job_id)
        && ledger.episode_id.as_deref() == Some(expected_episode_id)
    {
        Some(LostChargeResolution::AlreadyCharged)
    } else if ledger
        .episode_id
        .as_deref()
        .is_some_and(|episode_id| episode_id != expected_episode_id)
    {
        Some(LostChargeResolution::Defer)
    } else if ledger.state == "pending" && ledger.attempts == original_attempts {
        Some(LostChargeResolution::Retry)
    } else if ledger.state == "pending" && ledger.attempts > original_attempts {
        Some(LostChargeResolution::Defer)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
struct PendingObsoletion {
    job_id: String,
    occurrence_id: String,
    generation_id: String,
    source_object_id: String,
    source_revision: i64,
    source_artifact_digest: String,
    reason: &'static str,
}

impl PendingObsoletion {
    fn candidate(candidate: &DispatchCandidate, reason: &'static str) -> Self {
        Self {
            job_id: candidate.job_id.clone(),
            occurrence_id: candidate.occurrence_id.clone(),
            generation_id: candidate.generation_id.clone(),
            source_object_id: candidate.source_object_id.clone(),
            source_revision: candidate.source_revision,
            source_artifact_digest: candidate.source_artifact_digest.clone(),
            reason,
        }
    }
}

/// `run_pass` blocks and requires a multi-threaded Tokio runtime; `block_in_place` yields the worker while the pass waits.
pub struct EmbeddingDispatcher<'a> {
    kernel: &'a KernelStore,
    projection: &'a SearchProjection,
    synapse: &'a SynapseComponent,
    faults: Vec<DispatchFault>,
    scan_position: Option<ScanPosition>,
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
            faults: Vec::new(),
            scan_position: None,
        }
    }

    /// Arms `fault` for the next store call it names; each armed fault is consumed when it fires, so several can be armed for one pass.
    #[cfg(feature = "test-support")]
    pub fn inject_fault_for_test(&mut self, fault: DispatchFault) {
        self.faults.push(fault);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn eligibility_cursor_for_test(&self) -> Option<DispatchCursor> {
        self.scan_position
            .as_ref()
            .and_then(|position| position.cursor.clone())
    }

    fn take_fault(&mut self, fault: DispatchFault) -> bool {
        match self.faults.iter().position(|armed| *armed == fault) {
            Some(index) => {
                self.faults.remove(index);
                true
            }
            None => false,
        }
    }

    /// Runs one pass and reports what stopped it early, if anything; every confirmed job disposition reaches `observer`. An unknown terminal write is reconciled without a synthetic event because durable state cannot identify which writer performed the transition.
    /// `now` is one caller-defined logical-time snapshot for the entire pass;
    /// episode deadlines do not expire by wall clock while the pass runs.
    ///
    /// The pass sleeps while it polls, so it must not keep a runtime worker: on a worker it hands the worker's tasks off first, on a blocking thread it runs directly, and on a `current_thread` runtime it panics rather than starve the inference it waits for.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError::Retryable`] when the store refuses the lane binding or an eligible-row read, [`DispatchError::RetryableKernel`] when an eligibility read is busy or reaches its deadline, [`DispatchError::Quarantined`] once any writer quarantines the projection, and [`DispatchError::Kernel`] for other kernel failures.
    pub fn run_pass(
        &mut self,
        eligibility: EligibilityBinding<'_>,
        bounds: &DispatchBounds,
        budget: &EvalBudget,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        tokio::task::block_in_place(|| self.pass(eligibility, bounds, budget, now, observer))
    }

    fn pass(
        &mut self,
        eligibility: EligibilityBinding<'_>,
        bounds: &DispatchBounds,
        budget: &EvalBudget,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        self.check_quarantine()?;
        let cursor_binding = EligibilityCursorBinding {
            project: eligibility.project.clone(),
            destination: eligibility.destination,
        };
        if self.scan_position.as_ref().is_none_or(|position| {
            position.binding != cursor_binding
                || position.revisit_at.is_some_and(|revisit| revisit <= now)
        }) {
            self.scan_position = Some(ScanPosition {
                binding: cursor_binding.clone(),
                cursor: None,
                revisit_at: None,
            });
        }
        let Some(deadline) = budget.deadline() else {
            return Ok(Some(Blocked::UnboundedBudget));
        };
        if budget.is_exhausted() {
            return Ok(Some(Blocked::BudgetExhausted));
        }
        let lane = match serving(self.synapse.status()) {
            Ok(lane) => lane,
            Err(state) => return Ok(Some(Blocked::LaneUnavailable(state))),
        };
        let binding = lane_binding(&lane, self.synapse.host_incarnation());
        self.check_quarantine()?;
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
        // The binding's busy wait may have outlived the budget; a read under an exhausted budget selects rows the first `drive` would refuse anyway.
        if budget.is_exhausted() {
            return Ok(Some(Blocked::BudgetExhausted));
        }
        let page_limit = NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES)
            .expect("the kernel eligibility limit is nonzero");
        let position = self
            .scan_position
            .as_ref()
            .expect("the scan position is initialized above");
        let mut scan_cursor = position.cursor.clone();
        let mut safe_cursor = position.cursor.clone();
        let mut revisit_at = position.revisit_at;
        let mut action_found = false;
        let mut actions = 0;
        let mut selected = Vec::with_capacity(bounds.max_jobs.get());
        let mut terminal = Vec::with_capacity(bounds.max_jobs.get());
        for page_index in 0..MAX_ELIGIBILITY_PAGES_PER_PASS {
            let page = if self.take_fault(DispatchFault::RefuseEligibilityRead) {
                Err(SearchProjectionError::Store(storage::StoreError::Backend(
                    "database is locked".to_owned(),
                )))
            } else {
                self.projection
                    .read(|conn| open_job_candidates(conn, scan_cursor.as_ref(), page_limit, now))
            };
            let page = self.before_dispositions(page)?;
            if page.is_empty() {
                let restart_at_beginning = page_index == 0 && scan_cursor.is_some();
                scan_cursor = None;
                if !action_found {
                    safe_cursor = None;
                    revisit_at = None;
                }
                if restart_at_beginning {
                    continue;
                }
                break;
            }
            let short_page = page.len() < MAX_ELIGIBILITY_CANDIDATES;
            let (prepared, kernel_candidates) = prepare_candidates(&page)?;
            let verdicts = if kernel_candidates.is_empty() {
                Vec::new()
            } else {
                self.kernel
                    .judge_eligibility(
                        eligibility.project,
                        eligibility.destination,
                        &kernel_candidates,
                    )
                    .map_err(eligibility_error)?
                    .verdicts
            };
            let classifications = classify_candidates(prepared, verdicts)?;
            let mut processed = 0;
            for (candidate, classification) in page.iter().zip(classifications) {
                if actions == bounds.max_jobs.get() {
                    break;
                }
                processed += 1;
                scan_cursor = Some(candidate.cursor());
                let classification = match (candidate.readiness, classification) {
                    (_, CandidateClassification::Verdict(EligibilityVerdict::WrongScope)) => {
                        classification
                    }
                    (CandidateReadiness::Deferred { until }, _) => {
                        CandidateClassification::Deferred { until }
                    }
                    _ => classification,
                };
                match classification {
                    CandidateClassification::Deferred { until } => {
                        if !action_found {
                            safe_cursor = Some(candidate.cursor());
                            revisit_at = Some(revisit_at.map_or(until, |saved| saved.min(until)));
                        }
                    }
                    CandidateClassification::InvalidIdentity => {
                        action_found = true;
                        terminal.push(PendingObsoletion::candidate(candidate, "invalid_identity"));
                        actions += 1;
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::Ok) => {
                        action_found = true;
                        selected.push(candidate.job_id.clone());
                        actions += 1;
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::WrongScope) => {
                        if !action_found {
                            safe_cursor = Some(candidate.cursor());
                        }
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::Retracted) => {
                        action_found = true;
                        terminal.push(PendingObsoletion::candidate(candidate, "retracted"));
                        actions += 1;
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::Superseded) => {
                        action_found = true;
                        terminal.push(PendingObsoletion::candidate(candidate, "superseded"));
                        actions += 1;
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::Stale) => {
                        action_found = true;
                        terminal.push(PendingObsoletion::candidate(candidate, "stale"));
                        actions += 1;
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::Hidden) => {
                        action_found = true;
                        terminal.push(PendingObsoletion::candidate(candidate, "hidden"));
                        actions += 1;
                    }
                    CandidateClassification::Verdict(EligibilityVerdict::ProviderSensitive) => {
                        action_found = true;
                        terminal.push(PendingObsoletion::candidate(
                            candidate,
                            "provider_sensitive",
                        ));
                        actions += 1;
                    }
                }
            }
            if short_page && processed == page.len() {
                if !action_found {
                    safe_cursor = None;
                    revisit_at = None;
                }
                break;
            }
            if actions == bounds.max_jobs.get() {
                break;
            }
        }
        if let Some(blocked) = self.obsolete_candidates(&terminal, deadline, now, observer)? {
            return Ok(Some(blocked));
        }
        let pass = Pass {
            lane: &lane,
            binding: &binding,
            eligibility,
            bounds,
            budget,
            deadline,
            now,
        };
        for job_id in &selected {
            let job = self.projection.read(|conn| dispatch_job(conn, job_id, now));
            let Some(job) = self.before_dispositions(job)? else {
                continue;
            };
            self.check_quarantine()?;
            if let Some(blocked) = self.drive(&job, &pass, observer)? {
                return Ok(Some(blocked));
            }
        }
        self.scan_position = Some(ScanPosition {
            binding: cursor_binding,
            cursor: safe_cursor,
            revisit_at,
        });
        Ok(None)
    }

    fn before_dispositions<T>(
        &mut self,
        result: Result<T, SearchProjectionError>,
    ) -> Result<T, DispatchError> {
        // Storage failures before terminal writes are retryable because no disposition is uncertain.
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
        if pass.budget.is_exhausted() {
            return Ok(Some(Blocked::BudgetExhausted));
        }
        self.check_quarantine()?;
        // A generation the lane does not serve is a model mismatch: the work is obsolete, not failed.
        if !pass.binding.serves(&job.generation) {
            return self.obsolete_identity(
                job,
                "generation_mismatch",
                pass.deadline,
                pass.now,
                observer,
            );
        }
        let (mut host_job_id, item_id) = match &job.host_job_id {
            Some(host_job_id) if job.state == "admitted" => {
                if let Some(reason) = job.completion_refusal(pass.bounds.grant, pass.now) {
                    return self.stop(job, reason, pass.now, observer);
                }
                (host_job_id.clone(), job.item_id())
            }
            _ => {
                // Refuse the episode before host admission to avoid inference for refused work.
                if let Some(reason) = job.episode_refusal(pass.bounds.grant, pass.now) {
                    return self.stop(job, reason, pass.now, observer);
                }
                match self.admit(job, pass, observer)? {
                    Ok(admitted) => admitted,
                    Err(blocked) => return Ok(blocked),
                }
            }
        };
        pass.stage(job, Stage::Poll, observer);
        let mut readmitted = false;
        let mut started = Instant::now();
        loop {
            match self
                .synapse
                .poll_admitted(pass.lane, &host_job_id, &item_id, &job.text)
            {
                // The budget ended while the host still holds the job; the row stays admitted for a later pass to poll.
                PollOutcome::Pending { .. } if pass.budget.is_exhausted() => {
                    return Ok(Some(Blocked::BudgetExhausted));
                }
                PollOutcome::Pending { .. } if started.elapsed() < pass.bounds.result_wait => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                // This job's wait is over; the pass moves on and a later pass polls the held job.
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
                        .preflight_embedding_for_lane(pass.lane, &job.text)
                    {
                        Ok(admitted) => admitted,
                        Err(refusal) => {
                            return self.completion_preflight_refusal(job, refusal, pass, observer);
                        }
                    };
                    pass.stage(job, Stage::Publish, observer);
                    // `page` stays alive through publication so its lease keeps the result bytes counted while the vector is in use.
                    let input = match self.current_input(job, pass)? {
                        Ok(input) => input,
                        Err(blocked) => return Ok(blocked),
                    };
                    let publication = VectorPublication {
                        input,
                        generation: &job.generation,
                        vector,
                        input_bytes: job.text.len() as u64,
                        input_tokens: admitted.tokens().get(),
                    };
                    return self.publish(job, &host_job_id, &publication, pass, observer);
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
                PollOutcome::Restarted => {
                    match self.readmit(job, &item_id, &host_job_id, pass, observer)? {
                        Ok(rebound) => {
                            host_job_id = rebound;
                            readmitted = true;
                            started = Instant::now();
                            // The replacement job is polled under its own poll stage, so the trace keeps the admit-poll-publish order per host job.
                            pass.stage(job, Stage::Poll, observer);
                        }
                        Err(blocked) => return Ok(blocked),
                    }
                }
                PollOutcome::KeyMismatch | PollOutcome::BadCursor => {
                    return self.stop(job, "malformed_request", pass.now, observer);
                }
            }
        }
    }

    fn submit(
        &self,
        job: &DispatchJob,
        item_id: &str,
        pass: &Pass<'_>,
    ) -> Result<SubmitOutcome, SubmitFailure> {
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(SubmitFailure::Quarantined(quarantine));
        }
        let admitted = self
            .synapse
            .preflight_embedding_for_lane(pass.lane, &job.text)
            .map_err(SubmitFailure::Unavailable)?;
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(SubmitFailure::Quarantined(quarantine));
        }
        // Native work must not start under a budget that already ended; this is the last check before the host owns the call.
        if pass.budget.is_exhausted() {
            return Err(SubmitFailure::BudgetExhausted);
        }
        // The host item is the episode, so a new episode never reuses a job the table retains from a stopped one.
        self.synapse
            .submit_admitted(&admitted, item_id)
            .map_err(SubmitFailure::Unavailable)
    }

    fn admit(
        &mut self,
        job: &DispatchJob,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Result<(String, String), Option<Blocked>>, DispatchError> {
        let item_id = job.item_id();
        pass.stage(job, Stage::Admit, observer);
        let host_job_id = match self.submit(job, &item_id, pass) {
            Ok(SubmitOutcome::Queued { job_id }) => job_id,
            Ok(SubmitOutcome::Full) => {
                return self.retry(job, "admission_full", pass, observer).map(Err);
            }
            Ok(SubmitOutcome::Refused(reason)) => {
                return self.stop(job, reason, pass.now, observer).map(Err);
            }
            Ok(SubmitOutcome::Closing) => return Ok(Err(Some(Blocked::HostClosing))),
            Err(SubmitFailure::Unavailable(refusal)) => {
                return self
                    .dense_unavailable(job, refusal, pass, observer)
                    .map(Err);
            }
            Err(SubmitFailure::Quarantined(quarantine)) => {
                return Err(DispatchError::Quarantined(quarantine));
            }
            Err(SubmitFailure::BudgetExhausted) => {
                return Ok(Err(Some(Blocked::BudgetExhausted)));
            }
        };
        // The host owns native work from this point, whatever the charge that follows decides.
        observer(DispatchEvent::Submitted {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.clone(),
        });
        let charge = |conn: &storage::GuardedConn<'_>| {
            charge_admission(
                conn,
                &job.job_id,
                &item_id,
                pass.binding,
                &host_job_id,
                pass.bounds.grant,
                pass.now,
            )
        };
        let charged = if self.take_fault(DispatchFault::RefuseChargeStatement) {
            Err(SearchProjectionError::Projection(ProjectionError::Sqlite(
                "database is locked".to_owned(),
            )))
        } else {
            self.check_quarantine()?;
            let charged = self.projection.write(charge);
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
            // The charge's statement failed or its reply was lost: the row, not the error, says whether it committed. A pass that cannot learn the outcome stops dispatch without a word about the host job, whose claim stays open.
            Err(lost) => {
                let ledger = if self.take_fault(DispatchFault::RefuseLedgerRead) {
                    Err(SearchProjectionError::Store(storage::StoreError::Backend(
                        "database is locked".to_owned(),
                    )))
                } else {
                    self.projection.read(|conn| job_ledger(conn, &job.job_id))
                };
                match ledger {
                    Ok(Some(ledger)) => {
                        match lost_charge_resolution(job.attempts, &host_job_id, &item_id, &ledger)
                        {
                            Some(LostChargeResolution::AlreadyCharged) => Admission::AlreadyCharged,
                            Some(LostChargeResolution::Retry) => self.write(charge)?,
                            Some(LostChargeResolution::Defer) => return Ok(Err(None)),
                            None => {
                                return Err(self.enter_quarantine(QuarantineKind::Storage, &lost));
                            }
                        }
                    }
                    _ => return Err(self.enter_quarantine(QuarantineKind::Storage, &lost)),
                }
            }
        };
        let attempts = match charged {
            Admission::Charged { attempts } => attempts,
            Admission::AlreadyCharged => job.attempts + 1,
            Admission::EpisodeChanged => {
                self.orphan(job, &host_job_id, observer);
                return Ok(Err(None));
            }
            Admission::Stopped(reason) => {
                observer(DispatchEvent::Stopped {
                    job_id: job.job_id.clone(),
                    reason: reason.to_owned(),
                });
                return Ok(Err(None));
            }
            Admission::NotPending => {
                self.orphan(job, &host_job_id, observer);
                return Ok(Err(None));
            }
        };
        observer(DispatchEvent::Admitted {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.clone(),
            attempts,
        });
        Ok(Ok((host_job_id, item_id)))
    }

    /// Reports a host job the row will never ask for: its charge rolled back or the row changed underneath it.
    fn orphan(
        &self,
        job: &DispatchJob,
        host_job_id: &str,
        observer: &mut dyn FnMut(DispatchEvent),
    ) {
        observer(DispatchEvent::Orphaned {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.to_owned(),
        });
    }

    fn readmit(
        &mut self,
        job: &DispatchJob,
        item_id: &str,
        evicted: &str,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Result<String, Option<Blocked>>, DispatchError> {
        pass.stage(job, Stage::Admit, observer);
        let host_job_id = match self.submit(job, item_id, pass) {
            Ok(SubmitOutcome::Queued { job_id }) => job_id,
            Ok(SubmitOutcome::Full)
            | Err(SubmitFailure::Unavailable(DenseUnavailable::CountUnavailable(
                InferenceFailureKind::Execution,
            ))) => {
                return Ok(Err(None));
            }
            Ok(SubmitOutcome::Refused(reason)) => {
                return self.stop(job, reason, pass.now, observer).map(Err);
            }
            Ok(SubmitOutcome::Closing) => return Ok(Err(Some(Blocked::HostClosing))),
            Err(SubmitFailure::Unavailable(refusal)) => {
                return self
                    .dense_unavailable(job, refusal, pass, observer)
                    .map(Err);
            }
            Err(SubmitFailure::Quarantined(quarantine)) => {
                return Err(DispatchError::Quarantined(quarantine));
            }
            Err(SubmitFailure::BudgetExhausted) => {
                return Ok(Err(Some(Blocked::BudgetExhausted)));
            }
        };
        observer(DispatchEvent::Submitted {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.clone(),
        });
        let rebound =
            self.write(|conn| rebind_host_job(conn, &job.job_id, evicted, &host_job_id, pass.now))?;
        if !rebound {
            self.orphan(job, &host_job_id, observer);
            return Ok(Err(None));
        }
        observer(DispatchEvent::Admitted {
            job_id: job.job_id.clone(),
            host_job_id: host_job_id.clone(),
            attempts: job.attempts,
        });
        Ok(Ok(host_job_id))
    }

    fn current_input(
        &mut self,
        job: &DispatchJob,
        pass: &Pass<'_>,
    ) -> Result<Result<CurrentInputDescriptor, Option<Blocked>>, DispatchError> {
        let expectation = CurrentInputExpectation {
            object_id: job.source_object_id.clone(),
            source_revision: job.revision,
            occurrence_id: job.occurrence_id.clone(),
            payload_id: job.payload_id.clone(),
            artifact_digest: job.source_artifact_digest.clone(),
        };
        match self
            .kernel
            .guard_current_input(&expectation, pass.eligibility, pass.deadline)
        {
            Ok(Ok(guard)) => Ok(Ok(guard.descriptor().clone())),
            // `WrongScope` is a verdict about the binding, not the input, so the row is left open under the wrong binding rather than obsoleted.
            Ok(Err(stale))
                if matches!(
                    stale.reason(),
                    StaleInput::Ineligible(EligibilityVerdict::WrongScope)
                ) =>
            {
                Ok(Err(Some(Blocked::WrongScope)))
            }
            // The row stays open; the next scan's eligibility judgment retires it through the one obsoletion path.
            Ok(Err(_)) => Ok(Err(None)),
            Err(KernelError::Deadline) => Ok(Err(Some(Blocked::GuardDeadline))),
            Err(error) => Err(eligibility_error(error)),
        }
    }

    fn publish(
        &mut self,
        job: &DispatchJob,
        host_job_id: &str,
        publication: &VectorPublication<'_>,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        let mut publisher = EmbeddingPublisher::new(self.kernel, self.projection);
        let deadline = pass.deadline;
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
                    host_job_id: host_job_id.to_owned(),
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
            Err(PublicationError::WrongScope) => Ok(Some(Blocked::WrongScope)),
            Err(PublicationError::SearchDeadline) => Ok(Some(Blocked::SearchDeadline)),
            Err(PublicationError::LocalCommitUnresolved) => {
                Ok(Some(Blocked::LocalCommitUnresolved))
            }
            Err(PublicationError::NegativeTime { .. }) => Ok(Some(Blocked::PassTimeInvalid)),
            Err(PublicationError::ProjectionFenced) => Ok(Some(Blocked::ProjectionFenced)),
            // The publisher only hands back admission and identity refusals; its other classes already quarantined it.
            Err(PublicationError::Refused(error)) => match classify(&error) {
                Refusal::Identity => Ok(Some(Blocked::IdentityChanged)),
                // The row closed under another writer; the ledger already says so.
                _ => Ok(None),
            },
            Err(PublicationError::Quarantined(quarantine)) => {
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

    fn completion_preflight_refusal(
        &mut self,
        job: &DispatchJob,
        refusal: DenseUnavailable,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        match refusal {
            DenseUnavailable::CountUnavailable(InferenceFailureKind::Execution) => Ok(None),
            DenseUnavailable::CountUnavailable(
                InferenceFailureKind::Artifact | InferenceFailureKind::Invariant,
            ) => match serving(self.synapse.status()) {
                Err(state) => Ok(Some(Blocked::LaneUnavailable(state))),
                Ok(_) => Ok(None),
            },
            refusal => self.dense_unavailable(job, refusal, pass, observer),
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

    fn obsolete_candidates(
        &mut self,
        candidates: &[PendingObsoletion],
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        if candidates.is_empty() {
            return Ok(None);
        }
        self.check_quarantine()?;
        let outcomes = self.projection.write_within(deadline, |conn| {
            candidates
                .iter()
                .map(|candidate| {
                    obsolete_judged_job(
                        conn,
                        &candidate.occurrence_id,
                        &candidate.generation_id,
                        &candidate.source_object_id,
                        candidate.source_revision,
                        &candidate.source_artifact_digest,
                        now,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        });
        let outcomes = if self.take_fault(DispatchFault::LoseObsoletionReply) && outcomes.is_ok() {
            Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )))
        } else {
            outcomes
        };
        match outcomes {
            Ok(outcomes) => {
                for (candidate, outcome) in candidates.iter().zip(outcomes) {
                    if outcome == Obsoletion::Marked {
                        observer(DispatchEvent::Stopped {
                            job_id: candidate.job_id.clone(),
                            reason: candidate.reason.to_owned(),
                        });
                    }
                }
                Ok(None)
            }
            Err(SearchProjectionError::Projection(error)) => Err(match classify(&error) {
                Refusal::Integrity => self.enter_quarantine(QuarantineKind::Integrity, &error),
                Refusal::Storage => self.enter_quarantine(QuarantineKind::Storage, &error),
                Refusal::Admission | Refusal::Identity | Refusal::OperatorRepair => {
                    DispatchError::Retryable(SearchProjectionError::Projection(error))
                }
            }),
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                Ok(Some(Blocked::SearchDeadline))
            }
            Err(_) => self.reconcile_obsoletions(candidates),
        }
    }

    fn obsolete_identity(
        &mut self,
        job: &DispatchJob,
        reason: &'static str,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        self.obsolete_candidates(
            &[PendingObsoletion {
                job_id: job.job_id.clone(),
                occurrence_id: job.occurrence_id.clone(),
                generation_id: job.generation.generation_id.clone(),
                source_object_id: job.source_object_id.clone(),
                source_revision: job.revision,
                source_artifact_digest: job.source_artifact_digest.clone(),
                reason,
            }],
            deadline,
            now,
            observer,
        )
    }

    fn reconcile_obsoletions(
        &mut self,
        candidates: &[PendingObsoletion],
    ) -> Result<Option<Blocked>, DispatchError> {
        let statuses = self.projection.read(|conn| {
            candidates
                .iter()
                .map(|candidate| {
                    completion_status(conn, &candidate.occurrence_id, &candidate.generation_id)
                })
                .collect::<Result<Vec<_>, _>>()
        });
        match statuses {
            Ok(statuses) => {
                if statuses.iter().all(|status| {
                    !matches!(status.job_state.as_deref(), Some("pending" | "admitted"))
                }) {
                    Ok(None)
                } else {
                    Ok(Some(Blocked::LocalCommitUnresolved))
                }
            }
            Err(error) => Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
        }
    }

    /// Every disposition write that fails quarantines the projection: a refusal means the row's state no longer describes the work, and a store failure leaves it unknown whether the disposition committed.
    fn write<T>(
        &mut self,
        f: impl FnOnce(&storage::GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<T, DispatchError> {
        self.check_quarantine()?;
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
        DispatchError::Quarantined(self.projection.enter_quarantine(kind, error))
    }

    fn check_quarantine(&self) -> Result<(), DispatchError> {
        match self.projection.quarantine() {
            Some(quarantine) => Err(DispatchError::Quarantined(quarantine)),
            None => Ok(()),
        }
    }
}

/// What one pass holds fixed for every job it drives.
struct Pass<'a> {
    lane: &'a LaneInfo,
    binding: &'a LaneBinding,
    eligibility: EligibilityBinding<'a>,
    bounds: &'a DispatchBounds,
    budget: &'a EvalBudget,
    deadline: Instant,
    now: i64,
}

impl Pass<'_> {
    fn stage(&self, job: &DispatchJob, stage: Stage, observer: &mut dyn FnMut(DispatchEvent)) {
        observer(DispatchEvent::Stage {
            job_id: job.job_id.clone(),
            stage,
            deadline: self.deadline,
        });
    }
}

fn serving(status: SynapseStatus) -> Result<LaneInfo, LaneUnavailableState> {
    match status {
        SynapseStatus::Ready(lane) => Ok(lane),
        SynapseStatus::Starting => Err(LaneUnavailableState::Starting),
        SynapseStatus::Disabled { .. } => Err(LaneUnavailableState::Disabled),
        SynapseStatus::Failing { .. } => Err(LaneUnavailableState::Failing),
    }
}

fn eligibility_error(error: KernelError) -> DispatchError {
    match error {
        KernelError::Busy | KernelError::Deadline => DispatchError::RetryableKernel(error),
        _ => DispatchError::Kernel(error),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedCandidate {
    InvalidIdentity,
    NeedsVerdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateClassification {
    Deferred { until: i64 },
    InvalidIdentity,
    Verdict(EligibilityVerdict),
}

fn prepare_candidates(
    candidates: &[DispatchCandidate],
) -> Result<(Vec<PreparedCandidate>, Vec<EligibilityCandidate>), DispatchError> {
    let mut prepared = Vec::with_capacity(candidates.len());
    let mut kernel_candidates = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let kernel_candidate = EligibilityCandidate {
            object_id: candidate.source_object_id.clone(),
            source_revision: candidate.source_revision,
            artifact_digest: Some(candidate.source_artifact_digest.clone()),
        };
        match kernel_candidate.validate() {
            Ok(()) => {
                prepared.push(PreparedCandidate::NeedsVerdict);
                kernel_candidates.push(kernel_candidate);
            }
            Err(KernelError::InvalidInput) => prepared.push(PreparedCandidate::InvalidIdentity),
            Err(error) => return Err(DispatchError::Kernel(error)),
        }
    }
    Ok((prepared, kernel_candidates))
}

fn classify_candidates(
    prepared: Vec<PreparedCandidate>,
    verdicts: Vec<EligibilityVerdict>,
) -> Result<Vec<CandidateClassification>, DispatchError> {
    let expected = prepared
        .iter()
        .filter(|candidate| matches!(candidate, PreparedCandidate::NeedsVerdict))
        .count();
    let mut verdicts = exact_verdicts(verdicts, expected)?.into_iter();
    Ok(prepared
        .into_iter()
        .map(|candidate| match candidate {
            PreparedCandidate::InvalidIdentity => CandidateClassification::InvalidIdentity,
            PreparedCandidate::NeedsVerdict => CandidateClassification::Verdict(
                verdicts
                    .next()
                    .expect("verdict cardinality was checked before classification"),
            ),
        })
        .collect())
}

fn exact_verdicts(
    verdicts: Vec<EligibilityVerdict>,
    expected: usize,
) -> Result<Vec<EligibilityVerdict>, DispatchError> {
    if verdicts.len() != expected {
        return Err(DispatchError::Kernel(KernelError::AdmissionPolicy));
    }
    Ok(verdicts)
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

#[cfg(test)]
mod tests {
    use super::{
        DispatchError, LostChargeResolution, eligibility_error, exact_verdicts,
        lost_charge_resolution,
    };
    use kernel::{EligibilityVerdict, KernelError};
    use retrieval::dispatch::JobLedger;

    fn pending_ledger(attempts: u32) -> JobLedger {
        JobLedger {
            state: "pending".to_owned(),
            attempts,
            episode_id: None,
            episode_allowance: 0,
            episode_deadline: None,
            host_job_id: None,
            host_incarnation: None,
            last_failure_kind: None,
            stop_reason: None,
            authorization_ref: None,
        }
    }

    #[test]
    fn only_busy_and_deadline_eligibility_errors_are_retryable() {
        for error in [KernelError::Busy, KernelError::Deadline] {
            assert!(matches!(
                eligibility_error(error),
                DispatchError::RetryableKernel(actual) if actual == error
            ));
        }
        assert!(matches!(
            eligibility_error(KernelError::Held),
            DispatchError::Kernel(KernelError::Held)
        ));
    }

    #[test]
    fn eligibility_cardinality_mismatch_is_a_release_error() {
        assert!(matches!(
            exact_verdicts(vec![EligibilityVerdict::Ok], 2),
            Err(DispatchError::Kernel(KernelError::AdmissionPolicy))
        ));
    }

    #[test]
    fn lost_charge_reply_retries_only_the_same_attempt() {
        assert_eq!(
            lost_charge_resolution(2, "host", "job/1", &pending_ledger(2)),
            Some(LostChargeResolution::Retry)
        );
        let mut same_episode = pending_ledger(2);
        same_episode.episode_id = Some("episode".to_owned());
        same_episode.episode_allowance = 3;
        same_episode.episode_deadline = Some(100);
        assert_eq!(
            lost_charge_resolution(2, "host", "episode", &same_episode),
            Some(LostChargeResolution::Retry)
        );
        assert_eq!(
            lost_charge_resolution(2, "host", "episode", &pending_ledger(3)),
            Some(LostChargeResolution::Defer)
        );
        assert_eq!(
            lost_charge_resolution(2, "host", "episode", &pending_ledger(1)),
            None
        );
        let mut changed = same_episode.clone();
        changed.episode_id = Some("replacement".to_owned());
        assert_eq!(
            lost_charge_resolution(2, "host", "episode", &changed),
            Some(LostChargeResolution::Defer)
        );
        let mut admitted = same_episode;
        admitted.state = "admitted".to_owned();
        admitted.host_job_id = Some("host".to_owned());
        assert_eq!(
            lost_charge_resolution(2, "host", "episode", &admitted),
            Some(LostChargeResolution::AlreadyCharged)
        );
        assert_eq!(
            lost_charge_resolution(2, "other-host", "episode", &admitted),
            None
        );
        assert_eq!(
            lost_charge_resolution(2, "host", "replacement", &admitted),
            Some(LostChargeResolution::Defer)
        );
    }
}
