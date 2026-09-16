//! The blocking-pool placement of a transform pass.
//!
//! A pass's store work, its commit included, runs as units on the blocking pool. A unit is
//! one closure the request's runner executes through [`host_runtime::RequestCtx::run_blocking`],
//! so the host joins it on cancel and on route close and raises the panic-redaction guard on
//! its thread. The handler awaits each unit and decides what follows from what the unit
//! returned; a unit that cannot finish, because its thread panicked or the runtime stopped,
//! settles the request as an internal error rather than as an unavailable store.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use host_runtime::{BlockingWorkFailed, CancelSignal, RequestCtx};
use tokio::sync::OwnedSemaphorePermit;
#[cfg(any(test, feature = "test-support"))]
use tokio_util::sync::CancellationToken;

use crate::dispatch::PreparedOutcome;
use crate::metered_decode::ResidentMeter;
use crate::{
    HandlerCore, PassEnv, PreparedHistorySummarizerAction, RequestEntryProbe, TransformedPass,
};

/// Runs one unit of a pass off the async workers. The request's context is the runner in
/// production; tests supply one that runs the unit on a bare blocking thread.
pub(crate) trait UnitRunner: Send + Sync {
    /// Submits `work` when called, not when the future is first polled, and resolves with
    /// what the work returned or with why it produced nothing.
    fn run_unit(
        &self,
        work: Box<dyn FnOnce() -> UnitOutcome + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<UnitOutcome, BlockingWorkFailed>> + Send + 'static>>;

    /// Runs `work` on the same tracked primitive as a unit; the work reports through state it captures.
    fn run_step(
        &self,
        work: Box<dyn FnOnce() + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlockingWorkFailed>> + Send + 'static>>;

    /// The request's cancellation, for a unit to read at its head before it does any work.
    fn cancel_signal(&self) -> CancelSignal;
}

impl UnitRunner for RequestCtx {
    fn run_unit(
        &self,
        work: Box<dyn FnOnce() -> UnitOutcome + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<UnitOutcome, BlockingWorkFailed>> + Send + 'static>>
    {
        Box::pin(self.run_blocking(work))
    }

    fn run_step(
        &self,
        work: Box<dyn FnOnce() + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlockingWorkFailed>> + Send + 'static>> {
        Box::pin(self.run_blocking(work))
    }

    fn cancel_signal(&self) -> CancelSignal {
        RequestCtx::cancel_signal(self)
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub(crate) struct DetachedRunner {
    pub(crate) cancel: CancellationToken,
    pub(crate) cancel_before_step: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl UnitRunner for DetachedRunner {
    fn run_unit(
        &self,
        work: Box<dyn FnOnce() -> UnitOutcome + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<UnitOutcome, BlockingWorkFailed>> + Send + 'static>>
    {
        let joined = tokio::task::spawn_blocking(work);
        Box::pin(async move {
            match joined.await {
                Ok(outcome) => Ok(outcome),
                Err(join) if join.is_panic() => Err(BlockingWorkFailed::Panicked),
                Err(_) => Err(BlockingWorkFailed::RuntimeStopped),
            }
        })
    }

    fn run_step(
        &self,
        work: Box<dyn FnOnce() + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlockingWorkFailed>> + Send + 'static>> {
        if self.cancel_before_step {
            self.cancel.cancel();
        }
        let joined = tokio::task::spawn_blocking(work);
        Box::pin(async move {
            match joined.await {
                Ok(()) => Ok(()),
                Err(join) if join.is_panic() => Err(BlockingWorkFailed::Panicked),
                Err(_) => Err(BlockingWorkFailed::RuntimeStopped),
            }
        })
    }

    fn cancel_signal(&self) -> CancelSignal {
        CancelSignal::observing(self.cancel.clone())
    }
}

/// What a request brings to the routing arms: the daemon's shared state, the route, the
/// body's probe, the meter that charged the decode, and the runner for the pass's units.
pub(crate) struct PassEntry<'a> {
    pub(crate) core: &'a Arc<HandlerCore>,
    pub(crate) route: host_runtime::RouteHandle,
    pub(crate) probe: Option<&'a RequestEntryProbe>,
    pub(crate) meter: &'a ResidentMeter<'a>,
    pub(crate) runner: &'a dyn UnitRunner,
}

/// What a unit hands back to the handler.
pub(crate) enum UnitOutcome {
    /// The pass settled; this is its outcome.
    Terminal(PreparedOutcome),
    Continue(Box<PassContinuation>),
}

pub(crate) struct PassContinuation {
    pub(crate) pass: TransformedPass,
    pub(crate) action: PreparedHistorySummarizerAction,
    pub(crate) env: Arc<PassEnv>,
}

pub(crate) enum HistorySummarizerFollowup {
    Unchanged,
    Published,
    Failed,
}

/// The environment drops this hold after its request fields, so charges cover their lifetime.
pub(crate) struct PassHold {
    pub(crate) _charges: Vec<host_runtime::wire::ByteCharge>,
    pub(crate) _page_apply: Option<Arc<PageApplyGuard>>,
}

/// Keeps a paged session's `Applying` phase until the pass has settled or has been given up.
/// The page lane holds one clone to disarm after `finish_apply`; the pass holds another, so
/// the phase is released after the last unit's thread has finished, never while it may still
/// be committing.
pub(crate) struct PageApplyGuard {
    core: Arc<HandlerCore>,
    session_id: String,
    transform_id: String,
    finished: std::sync::atomic::AtomicBool,
}

impl PageApplyGuard {
    pub(crate) fn new(core: Arc<HandlerCore>, session_id: String, transform_id: String) -> Self {
        Self {
            core,
            session_id,
            transform_id,
            finished: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// `finish_apply` ran; the drop has nothing to release.
    pub(crate) fn disarm(&self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

impl Drop for PageApplyGuard {
    fn drop(&mut self) {
        if self.finished.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        self.core
            .transform_pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .release_applying(&self.session_id, &self.transform_id);
    }
}

/// The cap matches the serving runtime's worker count rather than the blocking pool's much larger limit.
/// Permits cover executing units and their store-lock waits, but not async history_summarizer waits.
pub(crate) const TRANSFORM_UNITS_AT_ONCE: usize = 4;

pub(crate) const TRANSFORM_WAITERS_AT_MOST: usize = 12;

pub(crate) const TRANSFORM_ADMISSION_PERMITS: usize =
    TRANSFORM_UNITS_AT_ONCE + TRANSFORM_WAITERS_AT_MOST;

/// The permit a unit holds while it runs.
pub(crate) type UnitPermit = OwnedSemaphorePermit;

pub(crate) type AdmissionPermit = OwnedSemaphorePermit;
