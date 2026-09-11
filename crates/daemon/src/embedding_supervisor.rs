//! Runs embedding maintenance as bounded slices under the daemon's shutdown token: a backfill pass over durable pending work, then an identity sweep, then a yield, so one maintenance kind never starves the other and no slice outlives its budget. The loop waits `idle` only when both kinds last found nothing to do, so a blocked lane cannot spin it and a dry sweep cannot throttle a backfill with a backlog.
//!
//! Every slice gets one `EvalBudget`: an absolute deadline derived once when the slice starts, and a sticky cancellation that shutdown raises. The dispatcher and the sweeper thread that same budget through admission, the result poll, the guard, and the reclamation write, so no stage renews it. A projection transaction's wait for the file is the store's own busy timeout, not the budget's, so a slice bound is the deadline plus that timeout. A slice runs on a tracked blocking thread; shutdown stops new slices, cancels the running one, and joins every tracked task. A native call that has not returned keeps its permit, its charges, and its result lease, and the join stays unresolved until it exits: grace expiry reports that state, it does not end it. A slice that panics is reported, not swallowed, and stops the supervisor. Quarantine from either maintenance kind stops the supervisor and retains every obligation for an operator. A read that fails before anything is decided is not terminal: the slice reports it and the loop runs the same kind again after the idle wait.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use host_runtime::synapse::SynapseComponent;
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, EligibilityBinding, KernelStore, ProjectScope};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::embedding_dispatch::{
    Blocked, DispatchBounds, DispatchError, DispatchEvent, DispatchFault, EmbeddingDispatcher,
};
use crate::identity_sweep::{IdentitySweeper, SweepError, SweepReport};
use crate::search_projection::SearchProjection;
use crate::search_writer::Quarantine;

/// The stores and lane one supervisor works against.
pub struct Maintained {
    pub kernel: Arc<KernelStore>,
    pub projection: Arc<SearchProjection>,
    pub synapse: Arc<SynapseComponent>,
    pub project: ProjectScope,
    pub destination: ArtifactDestination,
}

/// Finite bounds every slice runs under. `slice` is the absolute budget of one slice; `idle` is the wait once both a backfill and a sweep have found nothing to do.
#[derive(Debug, Clone, Copy)]
pub struct SliceBounds {
    pub dispatch: DispatchBounds,
    pub sweep_candidates: NonZeroUsize,
    pub slice: Duration,
    pub idle: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceKind {
    Backfill,
    Sweep,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SliceOutcome {
    /// The pass ended, drained or blocked. `admitted` and `published` count rows that moved toward a vector; `dispositions` counts rows the pass retried or stopped.
    Backfill {
        end: Option<Blocked>,
        admitted: usize,
        published: usize,
        dispositions: usize,
    },
    Sweep(SweepReport),
    /// The slice's first read failed before anything was decided; the message is the error's display. The next slice of the same kind runs the work again.
    ReadFailed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorEvent {
    /// A slice started under a budget with this deadline.
    SliceStarted { kind: SliceKind, deadline: Instant },
    SliceEnded {
        kind: SliceKind,
        outcome: SliceOutcome,
    },
    /// The supervisor stopped running slices; `Stop` names why.
    Stopped(Stop),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    Shutdown,
    Quarantined(Quarantine),
    /// A slice failed for a reason that is not a quarantine; the message is the error's display.
    Failed(String),
    /// A slice panicked; the message is the panic payload when it was a string.
    Panicked(String),
}

/// What shutdown found once every slice had joined and no admitted native call was still running.
#[derive(Debug, Clone, PartialEq)]
pub struct DrainReport {
    pub slices: usize,
    pub stop: Option<Stop>,
    /// Admitted jobs whose result the host holds but no pass published; their rows stay admitted for the next incarnation to reconcile.
    pub held_results: usize,
}

/// Shutdown is not resolved: a slice is still running after the grace, or native work this supervisor admitted has not exited the host. Nothing was released; a later call may find it joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "{slices} slice(s) still running and {native} native call(s) still owned after the grace period"
)]
pub struct Unresolved {
    pub slices: usize,
    pub native: usize,
}

/// How often shutdown re-reads the host's status for calls it still owns.
const NATIVE_EXIT_POLL: Duration = Duration::from_millis(20);

/// A host job submitted by this supervisor remains shutdown work while the host owns it.
struct HostJob {
    /// The durable job whose episode submitted this host job; one durable job owns a new host job per episode.
    job_id: String,
    /// Whether an admitted row owns this job's result: set by the `Admitted` that charged it, cleared when the row is retried or stopped. A submission whose charge rolled back or whose row changed underneath never sets it.
    result_expected: bool,
}

pub struct EmbeddingSupervisor {
    maintained: Arc<Maintained>,
    bounds: SliceBounds,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    events: UnboundedSender<SupervisorEvent>,
    tracker: TaskTracker,
    shutdown: CancellationToken,
    slices: AtomicUsize,
    /// Set by the first `run`; the loop it starts is the only one this supervisor ever runs.
    started: AtomicBool,
    stop: Mutex<Option<Stop>>,
    /// Host jobs this supervisor submitted and has not seen published, by host job identifier. An entry outlives its row's disposition: the host runs the call to completion whatever the row says, and a row reopened under a new episode submits a second host job beside the first.
    admitted: Mutex<BTreeMap<String, HostJob>>,
    /// Where the next identity sweep resumes its selection; `None` starts a pass over the table.
    sweep_cursor: Mutex<Option<String>>,
    panic_next_slice: AtomicBool,
    dispatch_fault: Mutex<Option<DispatchFault>>,
    #[cfg(feature = "test-support")]
    dispatch_tap: Mutex<Option<DispatchTap>>,
}

#[cfg(feature = "test-support")]
type DispatchTap = Arc<dyn Fn(&DispatchEvent) + Send + Sync>;

impl EmbeddingSupervisor {
    pub fn new(
        maintained: Maintained,
        bounds: SliceBounds,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        events: UnboundedSender<SupervisorEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            maintained: Arc::new(maintained),
            bounds,
            now,
            events,
            tracker: TaskTracker::new(),
            shutdown: CancellationToken::new(),
            slices: AtomicUsize::new(0),
            started: AtomicBool::new(false),
            stop: Mutex::new(None),
            admitted: Mutex::new(BTreeMap::new()),
            sweep_cursor: Mutex::new(None),
            panic_next_slice: AtomicBool::new(false),
            dispatch_fault: Mutex::new(None),
            #[cfg(feature = "test-support")]
            dispatch_tap: Mutex::new(None),
        })
    }

    /// Makes the next slice panic on its blocking thread, so panic reporting and the join can be exercised.
    #[cfg(feature = "test-support")]
    pub fn panic_next_slice_for_test(&self) {
        self.panic_next_slice.store(true, Ordering::SeqCst);
    }

    /// Arms `fault` on the next backfill slice's dispatcher.
    #[cfg(feature = "test-support")]
    pub fn inject_dispatch_fault_for_test(&self, fault: DispatchFault) {
        *self
            .dispatch_fault
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(fault);
    }

    /// Runs slices until shutdown or a stop. Must run inside a Tokio runtime; each slice is a tracked blocking task, and the loop yields between slices. The loop itself holds a tracker token, so `shutdown` cannot report a drain while a slice could still start. One supervisor runs one loop: a later call returns at once, whether the first loop is still running or has stopped, since two loops would interleave slices and their events over the same census and a stop is terminal.
    pub async fn run(self: Arc<Self>) {
        if self
            .started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let _running = self.tracker.token();
        let mut kind = SliceKind::Backfill;
        // Whether each kind's last slice found nothing to do; the loop waits only when both did, so a dry sweep never throttles a backfill with a backlog and a blocked backfill never spins while the sweep is also dry.
        let mut idle = Idle::default();
        loop {
            if self.shutdown.is_cancelled() {
                self.stop_with(Stop::Shutdown);
                return;
            }
            let budget = EvalBudget::new(
                Some(Instant::now() + self.bounds.slice),
                Arc::new(AtomicBool::new(false)),
            );
            let _ = self.events.send(SupervisorEvent::SliceStarted {
                kind,
                deadline: budget.deadline().expect("slice budgets carry a deadline"),
            });
            let mut slice = {
                let this = Arc::clone(&self);
                let budget = budget.clone();
                self.tracker
                    .spawn_blocking(move || this.slice(kind, &budget))
            };
            // Shutdown cancels the budget and then waits for the slice: the thread is never abandoned, and the slice sees the cancellation at its next job or poll.
            let joined = tokio::select! {
                biased;
                joined = &mut slice => joined,
                () = self.shutdown.cancelled() => {
                    budget.cancel();
                    slice.await
                }
            };
            self.slices.fetch_add(1, Ordering::SeqCst);
            match joined {
                Ok(Ok(outcome)) => {
                    idle.record(kind, idle_after(&outcome));
                    let _ = self
                        .events
                        .send(SupervisorEvent::SliceEnded { kind, outcome });
                }
                Ok(Err(stop)) => {
                    self.stop_with(stop);
                    return;
                }
                Err(join) if join.is_panic() => {
                    let payload = join.into_panic();
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "non-string panic payload".to_owned());
                    self.stop_with(Stop::Panicked(message));
                    return;
                }
                Err(join) => {
                    self.stop_with(Stop::Failed(join.to_string()));
                    return;
                }
            };
            kind = match kind {
                SliceKind::Backfill => SliceKind::Sweep,
                SliceKind::Sweep => SliceKind::Backfill,
            };
            if idle.both() {
                tokio::select! {
                    biased;
                    () = self.shutdown.cancelled() => {}
                    () = tokio::time::sleep(self.bounds.idle) => {}
                }
            } else {
                tokio::task::yield_now().await;
            }
        }
    }

    /// One slice on the blocking thread that owns it: a backfill pass or a sweep, under `budget`.
    fn slice(&self, kind: SliceKind, budget: &EvalBudget) -> Result<SliceOutcome, Stop> {
        if self.panic_next_slice.swap(false, Ordering::SeqCst) {
            panic!("maintenance slice panicked for the test");
        }
        let m = &self.maintained;
        match kind {
            SliceKind::Backfill => {
                let mut dispatcher = EmbeddingDispatcher::new(&m.kernel, &m.projection, &m.synapse);
                if let Some(fault) = self
                    .dispatch_fault
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    #[cfg(feature = "test-support")]
                    dispatcher.inject_fault_for_test(fault);
                    #[cfg(not(feature = "test-support"))]
                    let _ = fault;
                }
                let (mut admitted, mut published, mut dispositions) = (0, 0, 0);
                let end = dispatcher.run_pass(
                    EligibilityBinding {
                        project: &m.project,
                        destination: m.destination,
                    },
                    &self.bounds.dispatch,
                    budget,
                    (self.now)(),
                    &mut |event| {
                        #[cfg(feature = "test-support")]
                        if let Some(tap) = self
                            .dispatch_tap
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .as_ref()
                        {
                            tap(&event);
                        }
                        match event {
                            // The host owns native work from submission, whatever the charge decides.
                            DispatchEvent::Submitted {
                                job_id,
                                host_job_id,
                            } => {
                                self.lock_admitted().insert(
                                    host_job_id,
                                    HostJob {
                                        job_id,
                                        result_expected: false,
                                    },
                                );
                            }
                            // Only a charged admission binds the row to this job's result. The entry is recreated if a census ran between the submission and this charge and retired a result no row expected yet.
                            DispatchEvent::Admitted {
                                job_id,
                                host_job_id,
                                ..
                            } => {
                                admitted += 1;
                                self.lock_admitted()
                                    .entry(host_job_id)
                                    .or_insert(HostJob {
                                        job_id,
                                        result_expected: false,
                                    })
                                    .result_expected = true;
                            }
                            DispatchEvent::Published { host_job_id, .. } => {
                                published += 1;
                                self.lock_admitted().remove(&host_job_id);
                            }
                            // A retried or stopped job stays tracked: the host still runs its call, and only the host's status retires it. Every host job the row submitted loses its claim on a result.
                            DispatchEvent::Retried { job_id, .. }
                            | DispatchEvent::Stopped { job_id, .. } => {
                                dispositions += 1;
                                for job in self
                                    .lock_admitted()
                                    .values_mut()
                                    .filter(|job| job.job_id == job_id)
                                {
                                    job.result_expected = false;
                                }
                            }
                            _ => {}
                        }
                    },
                );
                // Host jobs the host has settled and no row expects leave the census here, so it holds only live obligations rather than every job ever submitted.
                self.native_census();
                match end {
                    Ok(end) => Ok(SliceOutcome::Backfill {
                        end,
                        admitted,
                        published,
                        dispositions,
                    }),
                    // A store refusal before any disposition left the ledger certain, so the next slice of this kind runs the pass again.
                    Err(DispatchError::Retryable(error)) => {
                        Ok(SliceOutcome::ReadFailed(error.to_string()))
                    }
                    Err(DispatchError::Quarantined(quarantine)) => {
                        Err(Stop::Quarantined(quarantine))
                    }
                    Err(other) => Err(Stop::Failed(other.to_string())),
                }
            }
            SliceKind::Sweep => {
                // The cursor outlives the sweeper: each sweep resumes where the last one ended, so identities held at the head of the table do not consume every sweep.
                let cursor = self.lock_sweep_cursor().take();
                let mut sweeper = IdentitySweeper::resuming(&m.projection, &m.synapse, cursor);
                let swept = sweeper.run_sweep(self.bounds.sweep_candidates, budget);
                *self.lock_sweep_cursor() = sweeper.cursor().map(str::to_owned);
                match swept {
                    Ok(report) => Ok(SliceOutcome::Sweep(report)),
                    Err(SweepError::Read(error)) => Ok(SliceOutcome::ReadFailed(error.to_string())),
                    Err(SweepError::Quarantined(quarantine)) => Err(Stop::Quarantined(quarantine)),
                }
            }
        }
    }

    fn stop_with(&self, stop: Stop) {
        let mut slot = self
            .stop
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            let _ = self.events.send(SupervisorEvent::Stopped(stop.clone()));
            *slot = Some(stop);
        }
    }

    /// Stops admission of new slices, cancels the running slice's budget, joins every tracked task, and then waits for native calls this supervisor admitted to exit the host, all within one `grace`. Repeated calls are idempotent: cancellation is sticky and a task joins once.
    ///
    /// # Errors
    ///
    /// Returns [`Unresolved`] when a tracked task is still running or a native call is still owned after `grace`; native work keeps every permit, charge, and lease, and a later call may still find it settled.
    pub async fn shutdown(&self, grace: Duration) -> Result<DrainReport, Unresolved> {
        let deadline = Instant::now() + grace;
        self.shutdown.cancel();
        self.tracker.close();
        if tokio::time::timeout_at(deadline.into(), self.tracker.wait())
            .await
            .is_err()
        {
            // The loop's own token is tracked too; everything beyond it is a slice thread.
            return Err(Unresolved {
                slices: self.tracker.len().saturating_sub(1),
                native: self.native_census().0,
            });
        }
        // Slices are joined; the host still owns whatever native calls they admitted. A call has no join handle, so its exit is observed by polling the host until the grace ends; a ready result is a held lease the next incarnation reconciles.
        let (native, held_results) = loop {
            let (native, held_results) = self.native_census();
            if native == 0 || Instant::now() >= deadline {
                break (native, held_results);
            }
            tokio::time::sleep(
                NATIVE_EXIT_POLL.min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        };
        if native > 0 {
            return Err(Unresolved { slices: 0, native });
        }
        // The first writer wins: this records `Shutdown` only for a loop the cancellation reached before it was first polled, so the report is final.
        self.stop_with(Stop::Shutdown);
        Ok(DrainReport {
            slices: self.slices.load(Ordering::SeqCst),
            stop: self
                .stop
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            held_results,
        })
    }

    /// `(running, held)` counts over the host jobs this supervisor submitted and has not seen published: calls the host still owns, and settled results an admitted row still expects. A job the host no longer holds, a failed job, and a result no row expects carry no obligation and are dropped as they are counted.
    fn native_census(&self) -> (usize, usize) {
        let mut admitted = self.lock_admitted();
        let (mut running, mut held) = (0, 0);
        admitted.retain(|host_job_id, job| {
            // Anything the table still holds that is not a settled result counts as owned native work, so an unknown status word fails closed.
            match self.maintained.synapse.job_status(host_job_id) {
                None | Some("failed") => false,
                Some("ready") if job.result_expected => {
                    held += 1;
                    true
                }
                Some("ready") => false,
                Some(_) => {
                    running += 1;
                    true
                }
            }
        });
        (running, held)
    }

    /// Host jobs the census still tracks.
    #[cfg(feature = "test-support")]
    pub fn tracked_host_jobs_for_test(&self) -> usize {
        self.lock_admitted().len()
    }

    /// Observes every dispatch event on the slice thread, before the supervisor acts on it.
    #[cfg(feature = "test-support")]
    pub fn tap_dispatch_events_for_test(
        &self,
        tap: impl Fn(&DispatchEvent) + Send + Sync + 'static,
    ) {
        *self
            .dispatch_tap
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(tap));
    }

    fn lock_admitted(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, HostJob>> {
        self.admitted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_sweep_cursor(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.sweep_cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The last verdict of each slice kind; both start idle so the first dry slice of either kind can wait.
struct Idle {
    backfill: bool,
    sweep: bool,
}

impl Default for Idle {
    fn default() -> Self {
        Self {
            backfill: true,
            sweep: true,
        }
    }
}

impl Idle {
    fn record(&mut self, kind: SliceKind, idle: bool) {
        match kind {
            SliceKind::Backfill => self.backfill = idle,
            SliceKind::Sweep => self.sweep = idle,
        }
    }

    fn both(&self) -> bool {
        self.backfill && self.sweep
    }
}

/// A slice that moved nothing is idle, whatever ended it: a blocked backfill cannot clear its blocker, and a budget that ran out before anything moved would run out again, so the next slice waits instead of spinning. A slice the budget cut short after some progress is not idle.
fn idle_after(outcome: &SliceOutcome) -> bool {
    match outcome {
        SliceOutcome::Backfill {
            admitted,
            published,
            dispositions,
            ..
        } => *admitted == 0 && *published == 0 && *dispositions == 0,
        SliceOutcome::Sweep(report) => report.jobs_reclaimed == 0 && report.vectors_reclaimed == 0,
        SliceOutcome::ReadFailed(_) => true,
    }
}
