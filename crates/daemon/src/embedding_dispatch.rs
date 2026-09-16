//! Drives durable pending embedding work through the in-process LocalEmbeddings job table to guarded completion.
//!
//! Pass order is lane binding, eligible-job selection, admission, result polling, and guarded publication.
//! `admit` passes one `item_id` to `submit` and `charge_admission`; `EpisodeChanged` defers the row.
//! The projection rows are the only queue. The job-table key binds the episode and text to the frozen lane identity, so duplicate admission and polling within that lane name the same work.
//! `bind_lane` returns rows owned by another host incarnation to pending and leaves their attempts unchanged.
//! Disposition failures enter `SearchProjection`'s shared quarantine, checked before host submission, terminal obsoletion, and disposition writes.
//! One `EvalBudget` bounds a pass: its absolute deadline is the kernel guard's deadline and caps every result wait, and its sticky cancellation stops the pass at the next job, the next poll, or the moment before native work would start. A started local transaction runs to its bounded end, and its wait for the projection file is bounded by the store's own busy timeout rather than the budget; a started native call is never preempted, so a cancelled pass leaves its admitted row for a later pass to poll.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use host_runtime::local_embeddings::{
    AdmittedInput, DenseUnavailable, EmbedTokens, InferenceFailureKind, LaneInfo,
    LaneUnavailableState, LocalEmbeddingsComponent, LocalEmbeddingsStatus, PollOutcome,
    SubmitOutcome, failure_is_permanent,
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
    /// The manifest's approved input envelope; a job outside it is refused before submission, whatever the lane itself would accept.
    pub input: InputEnvelope,
}

/// The largest input the manifest approves for one embedding: `embedding_input_bytes` and `embedding_input_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEnvelope {
    pub bytes: u64,
    pub tokens: u64,
}

impl InputEnvelope {
    /// No bound beyond the lane's own; for callers whose manifest does not apply.
    pub const UNBOUNDED: Self = Self {
        bytes: u64::MAX,
        tokens: u64::MAX,
    };
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

/// Where a pass over one eligibility binding resumes. A dispatcher built for the next slice takes the previous slice's position through [`EmbeddingDispatcher::resuming`], so a backlog longer than one pass's page bound is walked across slices instead of restarting from the top each time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPosition {
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
    local_embeddings: &'a LocalEmbeddingsComponent,
    faults: Vec<DispatchFault>,
    scan_position: Option<ScanPosition>,
}

impl<'a> EmbeddingDispatcher<'a> {
    pub fn new(
        kernel: &'a KernelStore,
        projection: &'a SearchProjection,
        local_embeddings: &'a LocalEmbeddingsComponent,
    ) -> Self {
        Self {
            kernel,
            projection,
            local_embeddings,
            faults: Vec::new(),
            scan_position: None,
        }
    }

    /// Continues from `position`; a position for another binding is replaced at the next pass.
    pub fn resuming(mut self, position: Option<ScanPosition>) -> Self {
        self.scan_position = position;
        self
    }

    /// The position the next pass over the same binding resumes from.
    pub fn scan_position(&self) -> Option<ScanPosition> {
        self.scan_position.clone()
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
        let pass_started = Instant::now();
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
        let lane = match serving(self.local_embeddings.status()) {
            Ok(lane) => lane,
            Err(state) => return Ok(Some(Blocked::LaneUnavailable(state))),
        };
        let binding = lane_binding(&lane, self.local_embeddings.host_incarnation());
        self.check_quarantine()?;
        let bound = if self.take_fault(DispatchFault::RefuseBinding) {
            Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )))
        } else {
            // The binding resets rows another incarnation held; it waits for the connection only until the budget's deadline, so a withdrawn grant or an ended slice never rebinds late.
            self.projection
                .write_within(deadline, |conn| bind_lane(conn, &binding, now))
        };
        // The bound is the budget's own deadline, so a connection still held then is the budget ending, with nothing rebound.
        if let Err(SearchProjectionError::Store(storage::StoreError::Deadline)) = &bound {
            return Ok(Some(Blocked::BudgetExhausted));
        }
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
                self.projection.read_within(deadline, |conn| {
                    open_job_candidates(conn, scan_cursor.as_ref(), page_limit, now)
                })
            };
            // The selection reads end at the budget's deadline, so a held connection ends the pass rather than outliving the slice.
            if let Err(SearchProjectionError::Store(storage::StoreError::Deadline)) = &page {
                return Ok(Some(Blocked::BudgetExhausted));
            }
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
                // A read ended by the budget is the pass ending at its budget, as at any other stage; a deadline the kernel reached on its own is retryable.
                match self.kernel.judge_eligibility_within_budget(
                    eligibility.project,
                    eligibility.destination,
                    &kernel_candidates,
                    budget,
                ) {
                    Ok(batch) => batch.verdicts,
                    Err(KernelError::Deadline) if budget.is_exhausted() => {
                        return Ok(Some(Blocked::BudgetExhausted));
                    }
                    Err(error) => return Err(eligibility_error(error)),
                }
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
        // The position is recorded as soon as the scan ends: a pass blocked or failed while acting on what it selected resumes behind the prefix it already walked, and the selected jobs lie past the safe cursor, so the next pass finds them again.
        self.scan_position = Some(ScanPosition {
            binding: cursor_binding,
            cursor: safe_cursor,
            revisit_at,
        });
        if let Some(blocked) =
            self.obsolete_candidates(&terminal, budget, deadline, now, observer)?
        {
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
            started: pass_started,
            row_deadline: deadline,
            slice_deadline: deadline,
        };
        for job_id in &selected {
            let job = self
                .projection
                .read_within(deadline, |conn| dispatch_job(conn, job_id, now));
            if let Err(SearchProjectionError::Store(storage::StoreError::Deadline)) = &job {
                return Ok(Some(Blocked::BudgetExhausted));
            }
            let Some(job) = self.before_dispositions(job)? else {
                continue;
            };
            self.check_quarantine()?;
            if let Some(blocked) = self.drive(&job, &pass, observer)? {
                return Ok(Some(blocked));
            }
        }
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
                pass.budget,
                pass.deadline,
                pass.now,
                observer,
            );
        }
        // The row's episode deadline as an instant, measured from the pass's clock reading at its start and never past the pass's own deadline. Admission, polling, re-admission, and publication all end there: a result that lands after it is left for a pass that stops the row, and a host restart during the wait renews the result wait but not this deadline.
        let remaining = u64::try_from(
            job.episode_deadline(pass.bounds.grant)
                .saturating_sub(pass.now),
        )
        .map(Duration::from_millis)
        .unwrap_or(Duration::ZERO);
        let row_deadline_at = pass.started.checked_add(remaining).unwrap_or(pass.deadline);
        let deadline_at = row_deadline_at.min(pass.deadline);
        let pass = &Pass {
            deadline: deadline_at,
            row_deadline: row_deadline_at,
            ..*pass
        };
        let (mut host_job_id, item_id) = match &job.host_job_id {
            Some(host_job_id) if job.state == "admitted" => {
                if let Some(reason) = job.completion_refusal(pass.bounds.grant, pass.now) {
                    return self.stop(job, reason, pass, observer);
                }
                (host_job_id.clone(), job.item_id())
            }
            _ => {
                // Refuse the episode before host admission to avoid inference for refused work.
                if let Some(reason) = job.episode_refusal(pass.bounds.grant, pass.now) {
                    return self.stop(job, reason, pass, observer);
                }
                // A row whose deadline passed while the pass reached it is left for a pass whose clock is past the deadline to stop it; no native call starts and no attempt is charged for it.
                if Instant::now() >= deadline_at {
                    return Ok(None);
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
                .local_embeddings
                .poll_admitted(pass.lane, &host_job_id, &item_id, &job.text)
            {
                // The budget ended while the host still holds the job; the row stays admitted for a later pass to poll.
                PollOutcome::Pending { .. } if pass.budget.is_exhausted() => {
                    return Ok(Some(Blocked::BudgetExhausted));
                }
                PollOutcome::Pending { .. }
                    if started.elapsed() < pass.bounds.result_wait
                        && Instant::now() < deadline_at =>
                {
                    std::thread::sleep(POLL_INTERVAL);
                }
                // This job's wait is over; the pass moves on and a later pass polls the held job.
                PollOutcome::Pending { .. } => return Ok(None),
                // A result ready only after the row's deadline is not published under it; the row stays admitted for a pass whose clock is past the deadline to stop it.
                PollOutcome::Page(_) if Instant::now() >= deadline_at => return Ok(None),
                PollOutcome::Page(page) => {
                    let Some((_, _, vector)) =
                        page.vectors.iter().find(|(id, _, _)| id == &item_id)
                    else {
                        return self.stop(job, "malformed_request", pass, observer);
                    };
                    // The stored input is judged again before its result is trusted: the exact count is what the completion is charged, and a lane or a manifest envelope that no longer admits the input cannot complete it.
                    let admitted = match self.admit_input(job, pass) {
                        Ok(admitted) => admitted,
                        Err(refusal) => {
                            return self.completion_preflight_refusal(job, refusal, pass, observer);
                        }
                    };
                    pass.stage(job, Stage::Publish, observer);
                    // The stage event is where the supervisor cancels the budget of a withdrawn grant; a vector is not committed under a manifest the grant no longer covers.
                    if pass.budget.is_exhausted() {
                        return Ok(Some(Blocked::BudgetExhausted));
                    }
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
                // A result past the row's deadline, whatever it says, is not the row's disposition: the row stays admitted for a pass whose clock is past the deadline to stop it.
                PollOutcome::Failed { .. } | PollOutcome::KeyMismatch | PollOutcome::BadCursor
                    if Instant::now() >= deadline_at =>
                {
                    return Ok(None);
                }
                PollOutcome::Failed { code, .. } if !failure_is_permanent(&code) => {
                    return self.retry(job, "execution_failure", pass, observer);
                }
                // A worker that found the lane down fails its job with the lane's reason; that is the lane's disposition, not the input's, so the row keeps its admitted state until a serving host reconciles it.
                PollOutcome::Failed { code, .. } => {
                    return match serving(self.local_embeddings.status()) {
                        Ok(_) => self.stop(job, &code, pass, observer),
                        Err(state) => Ok(Some(Blocked::LaneUnavailable(state))),
                    };
                }
                // One re-admission per pass keeps the pass finite, and none happens past the row's deadline: replacement inference for a row about to be stopped is not started.
                PollOutcome::Restarted if readmitted || Instant::now() >= deadline_at => {
                    return Ok(None);
                }
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
                    return self.stop(job, "malformed_request", pass, observer);
                }
            }
        }
    }

    /// Judges the row's input under the manifest's envelope beside the lane's own limits: bytes before the tokenizer runs, tokens once it has counted them. An admission and a completion judge the same way, so a result retained from a larger envelope is not published above the one now approved.
    fn admit_input<'t>(
        &self,
        job: &'t DispatchJob,
        pass: &Pass<'_>,
    ) -> Result<AdmittedInput<'t>, DenseUnavailable> {
        let envelope = pass.bounds.input;
        if job.text.len() as u64 > envelope.bytes {
            return Err(DenseUnavailable::ByteOverflow {
                bytes: job.text.len(),
                max_bytes: usize::try_from(envelope.bytes).unwrap_or(usize::MAX),
            });
        }
        let admitted = self
            .local_embeddings
            .preflight_embedding_for_lane(pass.lane, &job.text)?;
        if u64::from(admitted.tokens().get()) > envelope.tokens {
            return Err(DenseUnavailable::TokenOverflow {
                tokens: admitted.tokens(),
                max_tokens: EmbedTokens::new(u32::try_from(envelope.tokens).unwrap_or(u32::MAX)),
            });
        }
        Ok(admitted)
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
            .admit_input(job, pass)
            .map_err(SubmitFailure::Unavailable)?;
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(SubmitFailure::Quarantined(quarantine));
        }
        // Native work must not start under a budget that already ended, nor past the row's deadline the pass carries for this job; this is the last check before the host owns the call.
        if pass.budget.is_exhausted() || Instant::now() >= pass.deadline {
            return Err(SubmitFailure::BudgetExhausted);
        }
        // The host item is the episode, so a new episode never reuses a job the table retains from a stopped one.
        self.local_embeddings
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
                return self.stop(job, reason, pass, observer).map(Err);
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
            // The ledger write waits for the connection until the row's own deadline, past the slice's if need be, since the host owns native work from the submission on; a charge that could not begin by the row's deadline is not made, and the host job runs unowned.
            let charged = self.projection.write_within(pass.row_deadline, charge);
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
            // The connection was not acquired by the row's deadline, so nothing was written: the row stays pending for a pass that stops it, the host job it never owned is orphaned, and the pass ends, since the next job's reads would wait on the same holder.
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                self.orphan(job, &host_job_id, observer);
                return Ok(Err(Some(Blocked::SearchDeadline)));
            }
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
                    // The read waits for the connection only until the row's deadline, like the charge it reconciles.
                    self.projection
                        .read_within(pass.row_deadline, |conn| job_ledger(conn, &job.job_id))
                };
                match ledger {
                    // The outcome stays unknown at the row's deadline: the pass ends without a word about the host job, whose claim stays open for a later pass to reconcile.
                    Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                        return Ok(Err(Some(Blocked::SearchDeadline)));
                    }
                    Ok(Some(ledger)) => {
                        match lost_charge_resolution(job.attempts, &host_job_id, &item_id, &ledger)
                        {
                            Some(LostChargeResolution::AlreadyCharged) => Admission::AlreadyCharged,
                            // The ledger shows nothing charged, so the retry runs under the same row deadline as the first charge and orphans the host job when it cannot begin by then.
                            Some(LostChargeResolution::Retry) => {
                                match self.projection.write_within(pass.row_deadline, charge) {
                                    Ok(charged) => charged,
                                    Err(SearchProjectionError::Store(
                                        storage::StoreError::Deadline,
                                    )) => {
                                        self.orphan(job, &host_job_id, observer);
                                        return Ok(Err(Some(Blocked::SearchDeadline)));
                                    }
                                    Err(error) => return Err(self.store_failure(error)),
                                }
                            }
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
                return self.stop(job, reason, pass, observer).map(Err);
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
        // The rebind waits for the connection until the row's own deadline, as the charge does; a rebind that could not begin by then is not made, and the replacement job runs unowned.
        self.check_quarantine()?;
        let rebound = match self.projection.write_within(pass.row_deadline, |conn| {
            rebind_host_job(conn, &job.job_id, evicted, &host_job_id, pass.now)
        }) {
            Ok(rebound) => rebound,
            // As for the charge: the replacement job is orphaned and the pass ends at the held connection.
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                self.orphan(job, &host_job_id, observer);
                return Ok(Err(Some(Blocked::SearchDeadline)));
            }
            Err(error) => {
                return Err(match &error {
                    SearchProjectionError::Projection(refusal)
                        if !matches!(classify(refusal), Refusal::Storage) =>
                    {
                        self.enter_quarantine(QuarantineKind::Integrity, &error)
                    }
                    _ => self.enter_quarantine(QuarantineKind::Storage, &error),
                });
            }
        };
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
                self.stop(job, "invalid_vector", pass, observer)
            }
            Err(PublicationError::IdempotencyConflict) => {
                self.stop(job, "idempotency_conflict", pass, observer)
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
                self.stop(job, "artifact_invalid", pass, observer)
            }
            DenseUnavailable::EmptyInput | DenseUnavailable::ZeroTokens { .. } => {
                self.stop(job, "input_empty", pass, observer)
            }
            DenseUnavailable::ByteOverflow { .. } | DenseUnavailable::TokenOverflow { .. } => {
                self.stop(job, "input_over_limit", pass, observer)
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
            ) => match serving(self.local_embeddings.status()) {
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
        let retry_at = pass.now.saturating_add(pass.bounds.retry_after);
        let job_id = job.job_id.clone();
        match self.write_disposition(pass, |conn| {
            record_retry(conn, &job.job_id, kind, retry_at, pass.now)
        })? {
            Ok(Disposition::Retry) => observer(DispatchEvent::Retried { job_id, kind }),
            Ok(Disposition::Exhausted) => observer(DispatchEvent::Stopped {
                job_id,
                reason: EXHAUSTED.to_owned(),
            }),
            Ok(Disposition::NotOpen) => {}
            Err(blocked) => return Ok(Some(blocked)),
        }
        Ok(None)
    }

    fn stop(
        &mut self,
        job: &DispatchJob,
        reason: &str,
        pass: &Pass<'_>,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        let now = pass.now;
        match self.write_disposition(pass, |conn| stop_job(conn, &job.job_id, reason, now))? {
            Ok(true) => observer(DispatchEvent::Stopped {
                job_id: job.job_id.clone(),
                reason: reason.to_owned(),
            }),
            Ok(false) => {}
            Err(blocked) => return Ok(Some(blocked)),
        }
        Ok(None)
    }

    /// A retry or stop written within the job's disposition deadline. `Err(blocked)` means nothing was written and the caller ends the pass: the connection was still held at the deadline, or the budget was cancelled while it was awaited, so the transaction is not begun under a withdrawn grant. The row is left as it was for a later pass to judge under its own clock.
    fn write_disposition<T>(
        &mut self,
        pass: &Pass<'_>,
        f: impl FnOnce(&storage::GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<Result<T, Blocked>, DispatchError> {
        self.check_quarantine()?;
        let judged = |conn: &storage::GuardedConn<'_>| {
            if pass.budget.is_exhausted() {
                return Ok(None);
            }
            f(conn).map(Some)
        };
        match self
            .projection
            .write_within(pass.disposition_deadline(), judged)
        {
            Ok(Some(value)) => Ok(Ok(value)),
            Ok(None) => Ok(Err(Blocked::BudgetExhausted)),
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                Ok(Err(Blocked::SearchDeadline))
            }
            Err(error) => Err(self.store_failure(error)),
        }
    }

    /// Marks `candidates` obsolete in one transaction. `Ok(Some(blocked))` means no row was written: `write_within` did not acquire the connection before `deadline`, or `budget.is_exhausted()` held before the write began.
    fn obsolete_candidates(
        &mut self,
        candidates: &[PendingObsoletion],
        budget: &EvalBudget,
        deadline: Instant,
        now: i64,
        observer: &mut dyn FnMut(DispatchEvent),
    ) -> Result<Option<Blocked>, DispatchError> {
        if candidates.is_empty() {
            return Ok(None);
        }
        self.check_quarantine()?;
        let outcomes = self.projection.write_within(deadline, |conn| {
            if budget.is_exhausted() {
                return Ok(None);
            }
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
                .map(Some)
        });
        let outcomes = if self.take_fault(DispatchFault::LoseObsoletionReply) && outcomes.is_ok() {
            Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )))
        } else {
            outcomes
        };
        match outcomes {
            Ok(None) => Ok(Some(Blocked::BudgetExhausted)),
            Ok(Some(outcomes)) => {
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
            Err(_) => self.reconcile_obsoletions(candidates, deadline),
        }
    }

    fn obsolete_identity(
        &mut self,
        job: &DispatchJob,
        reason: &'static str,
        budget: &EvalBudget,
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
            budget,
            deadline,
            now,
            observer,
        )
    }

    /// Reads the rows after an obsoletion write whose reply was lost, within `deadline` like the write: a connection still held then leaves the outcome unresolved for the next pass, which reads the rows again.
    fn reconcile_obsoletions(
        &mut self,
        candidates: &[PendingObsoletion],
        deadline: Instant,
    ) -> Result<Option<Blocked>, DispatchError> {
        let statuses = self.projection.read_within(deadline, |conn| {
            candidates
                .iter()
                .map(|candidate| {
                    completion_status(conn, &candidate.occurrence_id, &candidate.generation_id)
                })
                .collect::<Result<Vec<_>, _>>()
        });
        match statuses {
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                Ok(Some(Blocked::LocalCommitUnresolved))
            }
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
    /// Quarantines the projection for a write's failure: an integrity refusal is the projection's, anything else the store's.
    fn store_failure(&mut self, error: SearchProjectionError) -> DispatchError {
        match &error {
            SearchProjectionError::Projection(refusal)
                if !matches!(classify(refusal), Refusal::Storage) =>
            {
                self.enter_quarantine(QuarantineKind::Integrity, &error)
            }
            _ => self.enter_quarantine(QuarantineKind::Storage, &error),
        }
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
    /// When the pass read `now`; the row deadlines it enforces are measured from here.
    started: Instant,
    /// The row's own episode deadline as an instant, not shortened by the pass's; the charge that follows a submission may run to it, since the host owns native work from the submission on.
    row_deadline: Instant,
    /// The slice's own deadline, kept beside the row-capped `deadline` so a row whose deadline has passed can still be stopped within the slice.
    slice_deadline: Instant,
}

impl Pass<'_> {
    /// Where a retry or stop for this job may still be written. A row whose deadline had already passed at the pass's clock reading is stopped within the slice's deadline, since that pass's clock is past it. Every other row's disposition ends at the row's deadline: a refusal learned after it, as when a token count answered late, is not recorded under the pass's earlier clock, and a later pass stops the row.
    fn disposition_deadline(&self) -> Instant {
        if self.row_deadline > self.started {
            self.deadline
        } else {
            self.slice_deadline
        }
    }
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

fn serving(status: LocalEmbeddingsStatus) -> Result<LaneInfo, LaneUnavailableState> {
    match status {
        LocalEmbeddingsStatus::Ready(lane) => Ok(lane),
        LocalEmbeddingsStatus::Starting => Err(LaneUnavailableState::Starting),
        LocalEmbeddingsStatus::Disabled { .. } => Err(LaneUnavailableState::Disabled),
        LocalEmbeddingsStatus::Failing { .. } => Err(LaneUnavailableState::Failing),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneIdentity {
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: Option<u32>,
    pub generation_epoch: u64,
}

impl From<&LaneInfo> for LaneIdentity {
    fn from(lane: &LaneInfo) -> Self {
        Self {
            embedding_model: lane.model.clone(),
            tokenizer_fingerprint: lane.fingerprint.clone(),
            vector_dimension: u32::try_from(lane.dims).ok(),
            generation_epoch: lane.table_epoch,
        }
    }
}

pub fn lane_binding(lane: &LaneInfo, host_incarnation: &str) -> LaneBinding {
    let identity = LaneIdentity::from(lane);
    LaneBinding {
        embedding_model: identity.embedding_model,
        bundle_fingerprint: identity.tokenizer_fingerprint,
        vector_dimension: identity.vector_dimension.unwrap_or(u32::MAX),
        table_epoch: identity.generation_epoch,
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
