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

/// The clone a request-scoped callee or blocking closure carries; every clone shares one flag and one deadline.
#[derive(Debug, Clone)]
pub struct SharedBudget {
    budget: EvalBudget,
    deadline: Instant,
    cancel: CancelSignal,
}

impl SharedBudget {
    pub fn eval(&self) -> &EvalBudget {
        &self.budget
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The host's cancellation is folded into the budget here, so a cooperative check sees it even before the guard drops.
    pub fn is_exhausted(&self) -> bool {
        if self.cancel.is_cancelled() {
            self.budget.cancel();
        }
        self.budget.is_exhausted()
    }

    /// Cancellation wins over the deadline when both hold, because a cancelled caller is not waiting for a deadline verdict.
    pub fn exhaustion(&self) -> Option<Exhaustion> {
        if self.cancel.is_cancelled() {
            return Some(Exhaustion::Cancelled);
        }
        (self.budget.is_exhausted() || Instant::now() >= self.deadline)
            .then_some(Exhaustion::Deadline)
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
            },
        })
    }

    /// Only request-scoped callees may hold the clone; longer-lived structures must not retain it.
    pub fn share(&self) -> SharedBudget {
        self.shared.clone()
    }
}

impl std::ops::Deref for RequestBudget {
    type Target = SharedBudget;

    fn deref(&self) -> &SharedBudget {
        &self.shared
    }
}

impl Drop for RequestBudget {
    fn drop(&mut self) {
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
        assert_eq!(budget.eval().deadline(), Some(budget.deadline()));
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
        let clone = budget.share();
        let mut stop = budget.stop_predicate();
        assert_eq!(clone.deadline(), budget.deadline());
        assert_eq!(clone.eval().deadline(), Some(budget.deadline()));
        assert!(!clone.is_exhausted());
        assert!(!stop());
        drop(budget);
        assert!(clone.is_exhausted());
        assert!(stop());
    }

    #[test]
    fn host_cancellation_is_observed_by_the_stop_predicate_and_reported_as_cancellation() {
        let (token, cancel) = signal();
        let budget =
            RequestBudget::derive(cancel, Some(60_000), Some(Duration::from_secs(60))).unwrap();
        let clone = budget.share();
        let mut stop = budget.stop_predicate();
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
