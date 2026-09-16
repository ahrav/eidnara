use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use host_runtime::{BlockingWorkFailed, CancelSignal};
use kernel::applicability::EvalBudget;

pub const REMAINING_MS_FIELD: &str = "remaining_ms";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BudgetRefusal {
    #[error("the route has no approved deadline ceiling")]
    CeilingUnapproved,
    /// A transport or host timeout never stands in for the caller's own remaining duration.
    #[error("the request carries no {REMAINING_MS_FIELD}")]
    RemainingMissing,
    #[error("the request's {REMAINING_MS_FIELD} is zero")]
    RemainingZero,
    #[error("the request was cancelled before its budget was derived")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingFailure {
    Panicked,
    RuntimeStopped,
    RouteClosing,
}

impl From<BlockingWorkFailed> for BlockingFailure {
    fn from(failed: BlockingWorkFailed) -> Self {
        match failed {
            BlockingWorkFailed::Panicked => Self::Panicked,
            BlockingWorkFailed::RuntimeStopped => Self::RuntimeStopped,
            BlockingWorkFailed::RouteClosing => Self::RouteClosing,
            _ => Self::RuntimeStopped,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exhaustion {
    Cancelled,
    Deadline,
}

/// The clone a request-scoped callee or blocking closure carries; every clone shares one flag, one deadline, and one record of why the flag was raised.
#[derive(Debug, Clone)]
pub struct SharedBudget {
    budget: EvalBudget,
    deadline: Instant,
    cancel: CancelSignal,
    /// Set when the guard is dropped, so a drop-cancel is classified as cancellation rather than as a deadline the clock never reached.
    guard_dropped: Arc<AtomicBool>,
}

impl SharedBudget {
    pub fn eval(&self) -> &EvalBudget {
        &self.budget
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    fn cancelled(&self) -> bool {
        self.guard_dropped.load(Ordering::SeqCst) || self.cancel.is_cancelled()
    }

    /// Cancellation wins over the deadline when both hold, because a cancelled caller is not waiting for a deadline verdict.
    /// The host's cancellation is folded into the flag here, so callees polling only the `EvalBudget` stop as well.
    pub fn exhaustion(&self) -> Option<Exhaustion> {
        // The interrupt is read before the reason. The guard's drop publishes the reason, fences,
        // then raises the interrupt, so a poll that sees the interrupt and then reads the reason
        // after this acquire fence sees the drop; a poll that read the reason first could see the
        // interrupt land in between and report a deadline the clock never reached.
        let exhausted = self.budget.is_exhausted();
        std::sync::atomic::fence(Ordering::Acquire);
        if self.cancelled() {
            self.budget.cancel();
            return Some(Exhaustion::Cancelled);
        }
        exhausted.then_some(Exhaustion::Deadline)
    }

    pub fn is_exhausted(&self) -> bool {
        self.exhaustion().is_some()
    }

    /// Returns `true` once cancellation or the request deadline should stop SQLite work or an acquisition wait.
    pub fn stop_predicate(&self) -> impl FnMut() -> bool + Send + 'static {
        let shared = self.clone();
        move || shared.is_exhausted()
    }
}

/// Dropping the guard cancels the budget, so a handler future the host aborts still stops its blocking work.
#[derive(Debug)]
pub struct RequestBudget {
    shared: SharedBudget,
}

impl RequestBudget {
    /// `remaining_ms` is the caller's own remaining duration from the request body, clamped to the route's approved `ceiling`.
    /// The deadline starts when `derive` runs, excluding prior host admission time, and no `Instant` crosses a process.
    pub fn derive(
        cancel: CancelSignal,
        remaining_ms: Option<u64>,
        ceiling: Option<Duration>,
    ) -> Result<Self, BudgetRefusal> {
        let ceiling = ceiling.ok_or(BudgetRefusal::CeilingUnapproved)?;
        let remaining = remaining_ms.ok_or(BudgetRefusal::RemainingMissing)?;
        if remaining == 0 {
            return Err(BudgetRefusal::RemainingZero);
        }
        if cancel.is_cancelled() {
            return Err(BudgetRefusal::Cancelled);
        }
        let deadline = Instant::now() + Duration::from_millis(remaining).min(ceiling);
        Ok(Self {
            shared: SharedBudget {
                budget: EvalBudget::new(Some(deadline), Default::default()),
                deadline,
                cancel,
                guard_dropped: Default::default(),
            },
        })
    }

    /// Only request-scoped callees and the request's own blocking closures may hold a clone; longer-lived structures must not retain one.
    pub fn shared(&self) -> &SharedBudget {
        &self.shared
    }

    pub fn deadline(&self) -> Instant {
        self.shared.deadline
    }

    pub fn exhaustion(&self) -> Option<Exhaustion> {
        self.shared.exhaustion()
    }

    pub fn is_exhausted(&self) -> bool {
        self.shared.is_exhausted()
    }
}

impl Drop for RequestBudget {
    fn drop(&mut self) {
        // This fence pairs with the acquire fence in `SharedBudget::exhaustion`, which reads the
        // interrupt before the reason: a poll that observes the relaxed interrupt store below is
        // then guaranteed to read `guard_dropped` as set, so a drop-cancel is never reported as a
        // deadline.
        self.shared.guard_dropped.store(true, Ordering::SeqCst);
        std::sync::atomic::fence(Ordering::SeqCst);
        self.shared.budget.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn signal() -> (CancellationToken, CancelSignal) {
        let token = CancellationToken::new();
        (token.clone(), CancelSignal::observing(token))
    }

    #[test]
    fn the_remaining_duration_is_clamped_to_the_approved_ceiling() {
        let (_, cancel) = signal();
        let before = Instant::now();
        let budget =
            RequestBudget::derive(cancel, Some(60_000), Some(Duration::from_secs(5))).unwrap();
        let after = Instant::now();
        assert!(budget.deadline() <= after + Duration::from_secs(5));
        assert!(budget.deadline() >= before + Duration::from_secs(5));
        assert_eq!(budget.shared().eval().deadline(), Some(budget.deadline()));
        assert_eq!(budget.exhaustion(), None);
        assert!(!budget.is_exhausted());
    }

    #[test]
    fn a_request_without_an_approved_ceiling_or_a_remaining_duration_is_refused() {
        let (token, cancel) = signal();
        assert_eq!(
            RequestBudget::derive(cancel.clone(), Some(10), None).unwrap_err(),
            BudgetRefusal::CeilingUnapproved
        );
        assert_eq!(
            RequestBudget::derive(cancel.clone(), None, Some(Duration::from_secs(1))).unwrap_err(),
            BudgetRefusal::RemainingMissing
        );
        assert_eq!(
            RequestBudget::derive(cancel.clone(), Some(0), Some(Duration::from_secs(1)))
                .unwrap_err(),
            BudgetRefusal::RemainingZero
        );
        token.cancel();
        assert_eq!(
            RequestBudget::derive(cancel, Some(10), Some(Duration::from_secs(1))).unwrap_err(),
            BudgetRefusal::Cancelled
        );
    }

    #[test]
    fn dropping_the_guard_cancels_every_clone_and_the_stop_predicate() {
        let (_, cancel) = signal();
        let budget =
            RequestBudget::derive(cancel, Some(60_000), Some(Duration::from_secs(60))).unwrap();
        let clone = budget.shared().clone();
        let mut stop = budget.shared().stop_predicate();
        assert_eq!(clone.deadline(), budget.deadline());
        assert_eq!(clone.eval().deadline(), Some(budget.deadline()));
        assert!(!clone.is_exhausted());
        assert!(!stop());
        drop(budget);
        assert!(clone.is_exhausted());
        assert!(stop());
        assert_eq!(clone.exhaustion(), Some(Exhaustion::Cancelled));
    }

    #[test]
    fn host_cancellation_is_observed_by_the_stop_predicate_and_reported_as_cancellation() {
        let (token, cancel) = signal();
        let budget =
            RequestBudget::derive(cancel, Some(60_000), Some(Duration::from_secs(60))).unwrap();
        let clone = budget.shared().clone();
        let mut stop = budget.shared().stop_predicate();
        token.cancel();
        assert!(stop());
        assert!(clone.is_exhausted());
        assert_eq!(budget.exhaustion(), Some(Exhaustion::Cancelled));
    }

    #[test]
    fn an_elapsed_deadline_is_reported_as_deadline() {
        let (_, cancel) = signal();
        let budget = RequestBudget::derive(cancel, Some(1), Some(Duration::from_secs(60))).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(budget.is_exhausted());
        assert_eq!(budget.exhaustion(), Some(Exhaustion::Deadline));
    }

    /// A poller whose `exhaustion` call straddles the guard's drop must still read the drop as
    /// cancellation: the deadline is an hour away, so `Deadline` can only come from observing the
    /// interrupt flag without the reason that was published before it. The window is two atomic
    /// loads wide; each trial drops only after the poller has completed one `None` poll, so the
    /// drop lands while the poller is live, and repeated trials raise the chance that it lands
    /// inside the window.
    #[test]
    fn a_poll_that_straddles_the_guard_drop_never_reports_a_deadline() {
        use std::sync::atomic::AtomicUsize;
        const TRIALS: usize = 400;
        let mut verdicts = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let (_, cancel) = signal();
            let budget =
                RequestBudget::derive(cancel, Some(3_600_000), Some(Duration::from_secs(3_600)))
                    .unwrap();
            let clone = budget.shared().clone();
            let polls = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&polls);
            let poller = std::thread::spawn(move || {
                loop {
                    let reason = clone.exhaustion();
                    counted.fetch_add(1, Ordering::SeqCst);
                    if let Some(reason) = reason {
                        return reason;
                    }
                }
            });
            let give_up = Instant::now() + Duration::from_secs(5);
            while polls.load(Ordering::SeqCst) == 0 {
                assert!(
                    Instant::now() < give_up,
                    "the poller never completed a poll"
                );
                std::hint::spin_loop();
            }
            drop(budget);
            verdicts.push(poller.join().unwrap());
        }
        let deadlines = verdicts
            .iter()
            .filter(|reason| **reason == Exhaustion::Deadline)
            .count();
        assert_eq!(
            deadlines, 0,
            "{deadlines} of {TRIALS} polls read the guard's drop as a deadline"
        );
    }

    #[test]
    fn blocking_failures_map_onto_the_daemon_classes() {
        assert_eq!(
            BlockingFailure::from(BlockingWorkFailed::Panicked),
            BlockingFailure::Panicked
        );
        assert_eq!(
            BlockingFailure::from(BlockingWorkFailed::RuntimeStopped),
            BlockingFailure::RuntimeStopped
        );
        assert_eq!(
            BlockingFailure::from(BlockingWorkFailed::RouteClosing),
            BlockingFailure::RouteClosing
        );
    }
}
