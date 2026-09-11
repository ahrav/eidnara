//! Runs embedding maintenance as bounded slices under the daemon's shutdown token: a backfill pass over durable pending work, then an identity sweep, then a yield, so one maintenance kind never starves the other and no slice outlives its budget.
//!
//! Every slice gets one `EvalBudget`: an absolute deadline derived once when the slice starts, and a sticky cancellation that shutdown raises. The dispatcher and the sweeper thread that same budget through admission, the result poll, the guard, and the reclamation write, so no stage renews it. A projection transaction's wait for the file is the store's own busy timeout, not the budget's, so a slice bound is the deadline plus that timeout. A slice runs on a tracked blocking thread; shutdown stops new slices, cancels the running one, and joins every tracked task. A native call that has not returned keeps its permit, its charges, and its result lease, and the join stays unresolved until it exits: grace expiry reports that state, it does not end it. A slice that panics is reported, not swallowed, and stops the supervisor. Quarantine from either maintenance kind stops the supervisor and retains every obligation for an operator.

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
    Blocked, DispatchBounds, DispatchError, DispatchEvent, EmbeddingDispatcher,
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

/// Finite bounds every slice runs under. `slice` is the absolute budget of one slice; `idle` is the wait after a slice that found nothing to do.
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
    /// The pass ended, drained or blocked; `admitted` and `published` count its dispositions.
    Backfill {
        end: Option<Blocked>,
        admitted: usize,
        published: usize,
    },
    Sweep(SweepReport),
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

pub struct EmbeddingSupervisor {
    maintained: Arc<Maintained>,
    bounds: SliceBounds,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    events: UnboundedSender<SupervisorEvent>,
    tracker: TaskTracker,
    shutdown: CancellationToken,
    slices: AtomicUsize,
    stop: Mutex<Option<Stop>>,
    /// Host jobs this supervisor admitted and has not seen published, by durable job identifier: the physical work its shutdown must account for.
    admitted: Mutex<BTreeMap<String, String>>,
    panic_next_slice: AtomicBool,
}

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
            stop: Mutex::new(None),
            admitted: Mutex::new(BTreeMap::new()),
            panic_next_slice: AtomicBool::new(false),
        })
    }

    /// Makes the next slice panic on its blocking thread, so panic reporting and the join can be exercised.
    #[cfg(feature = "test-support")]
    pub fn panic_next_slice_for_test(&self) {
        self.panic_next_slice.store(true, Ordering::SeqCst);
    }

    /// Runs slices until shutdown or a stop. Must run inside a Tokio runtime; each slice is a tracked blocking task, and the loop yields between slices. The loop itself holds a tracker token, so `shutdown` cannot report a drain while a slice could still start.
    pub async fn run(self: Arc<Self>) {
        let _running = self.tracker.token();
        let mut kind = SliceKind::Backfill;
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
            let idle = match joined {
                Ok(Ok(outcome)) => {
                    let idle = idle_after(&outcome);
                    let _ = self
                        .events
                        .send(SupervisorEvent::SliceEnded { kind, outcome });
                    idle
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
            if idle {
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
                let (mut admitted, mut published) = (0, 0);
                let end = dispatcher
                    .run_pass(
                        EligibilityBinding {
                            project: &m.project,
                            destination: m.destination,
                        },
                        &self.bounds.dispatch,
                        budget,
                        (self.now)(),
                        &mut |event| match event {
                            // The host owns native work from submission, whatever the charge decides.
                            DispatchEvent::Submitted {
                                job_id,
                                host_job_id,
                            } => {
                                self.lock_admitted().insert(job_id, host_job_id);
                            }
                            DispatchEvent::Admitted { .. } => admitted += 1,
                            DispatchEvent::Published { job_id, .. } => {
                                published += 1;
                                self.lock_admitted().remove(&job_id);
                            }
                            // A retried or stopped row no longer expects its host job's result.
                            DispatchEvent::Retried { job_id, .. }
                            | DispatchEvent::Stopped { job_id, .. } => {
                                self.lock_admitted().remove(&job_id);
                            }
                            _ => {}
                        },
                    )
                    .map_err(|error| match error {
                        DispatchError::Quarantined(quarantine) => Stop::Quarantined(quarantine),
                        other => Stop::Failed(other.to_string()),
                    })?;
                Ok(SliceOutcome::Backfill {
                    end,
                    admitted,
                    published,
                })
            }
            SliceKind::Sweep => {
                let mut sweeper = IdentitySweeper::new(&m.projection, &m.synapse);
                sweeper
                    .run_sweep(self.bounds.sweep_candidates, budget)
                    .map(SliceOutcome::Sweep)
                    .map_err(|error| match error {
                        SweepError::Quarantined(quarantine) => Stop::Quarantined(quarantine),
                        other => Stop::Failed(other.to_string()),
                    })
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

    /// Stops admission of new slices, cancels the running slice's budget, and joins every tracked task within `grace`. Repeated calls are idempotent: cancellation is sticky and a task joins once.
    ///
    /// # Errors
    ///
    /// Returns [`Unresolved`] when a tracked task is still running after `grace`; its native work keeps every permit, charge, and lease, and a later call may still join it.
    pub async fn shutdown(&self, grace: Duration) -> Result<DrainReport, Unresolved> {
        self.shutdown.cancel();
        self.tracker.close();
        if tokio::time::timeout(grace, self.tracker.wait())
            .await
            .is_err()
        {
            // The loop's own token is tracked too; everything beyond it is a slice thread.
            return Err(Unresolved {
                slices: self.tracker.len().saturating_sub(1),
                native: self.native_census().0,
            });
        }
        // Slices are joined; the host still owns whatever native calls they admitted. A running call keeps shutdown unresolved; a ready result is a held lease the next incarnation reconciles.
        let (native, held_results) = self.native_census();
        if native > 0 {
            return Err(Unresolved { slices: 0, native });
        }
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

    /// `(running, ready)` counts over the host jobs this supervisor admitted and has not seen published.
    fn native_census(&self) -> (usize, usize) {
        let admitted = self.lock_admitted();
        admitted
            .values()
            .fold((0, 0), |(running, ready), host_job_id| {
                // Anything the table still holds that is not a settled result counts as owned native work, so an unknown status word fails closed.
                match self.maintained.synapse.job_status(host_job_id) {
                    None | Some("failed") => (running, ready),
                    Some("ready") => (running, ready + 1),
                    Some(_) => (running + 1, ready),
                }
            })
    }

    fn lock_admitted(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.admitted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A slice that found no work admits an idle wait; one that made progress yields and runs the next kind at once.
fn idle_after(outcome: &SliceOutcome) -> bool {
    match outcome {
        SliceOutcome::Backfill {
            admitted,
            published,
            end,
        } => *admitted == 0 && *published == 0 && end.is_none(),
        SliceOutcome::Sweep(report) => report.jobs_reclaimed == 0 && report.candidates == 0,
    }
}
