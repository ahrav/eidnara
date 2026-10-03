//! The one native inference slot, shared by queries and background batch texts.
//!
//! Waiters queue FIFO within their class. A free slot goes to the oldest query first, except that after
//! [`QUERY_STREAK`] consecutive query grants made while background work waits, the next grant goes to
//! the oldest background waiter. At most `max_waiting_queries` queries wait at once; the holder is not a
//! waiter. Every grant decision, waiter removal, and close happens under one lock, so a grant is handed to
//! exactly one waiter or returned to the slot exactly once.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{Notify, oneshot};

/// Query grants made in a row while background work waits before a background text is served.
pub(crate) const QUERY_STREAK: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Query,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// `max_waiting_queries` queries already wait.
    QueueFull,
    /// The scheduler is closed for shutdown.
    Closed,
}

struct Waiter {
    id: u64,
    granted: Arc<AtomicBool>,
    wake: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    held: bool,
    closed: bool,
    queries: VecDeque<Waiter>,
    background: VecDeque<Waiter>,
    streak: u32,
    next_id: u64,
}

impl State {
    /// The waiter the free slot goes to, with the streak updated for that grant.
    fn next(&mut self) -> Option<Waiter> {
        let background_waits = !self.background.is_empty();
        if background_waits && (self.queries.is_empty() || self.streak >= QUERY_STREAK) {
            self.streak = 0;
            return self.background.pop_front();
        }
        let query = self.queries.pop_front()?;
        self.streak = if background_waits { self.streak + 1 } else { 0 };
        Some(query)
    }
}

pub(crate) struct Scheduler {
    state: Mutex<State>,
    idle: Notify,
    max_waiting_queries: usize,
}

/// The slot, held until dropped; dropping it hands the slot to the next waiter.
#[must_use = "dropping a grant releases the inference slot"]
pub(crate) struct Grant {
    scheduler: Arc<Scheduler>,
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.scheduler.release();
    }
}

/// A queued waiter; dropping it before the grant is received removes it, or returns a grant it was handed.
struct Waiting<'a> {
    scheduler: &'a Arc<Scheduler>,
    id: u64,
    class: Class,
    granted: Arc<AtomicBool>,
    received: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self.received {
            return;
        }
        let mut state = self.scheduler.lock();
        let queue = match self.class {
            Class::Query => &mut state.queries,
            Class::Background => &mut state.background,
        };
        if let Some(index) = queue.iter().position(|waiter| waiter.id == self.id) {
            queue.remove(index);
            return;
        }
        // Absent from its queue: either close dropped it or a release handed it the slot it never took.
        let handed = self.granted.load(Ordering::Acquire);
        drop(state);
        if handed {
            self.scheduler.release();
        }
    }
}

impl Scheduler {
    pub(crate) fn new(max_waiting_queries: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            idle: Notify::new(),
            max_waiting_queries,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for the slot in `class`'s queue. A query is refused at once when `max_waiting_queries` queries already wait; dropping the future leaves no waiter and no held slot behind.
    pub(crate) async fn acquire(self: &Arc<Self>, class: Class) -> Result<Grant, Refusal> {
        let (id, granted, wake) = {
            let mut state = self.lock();
            if state.closed {
                return Err(Refusal::Closed);
            }
            if !state.held && state.queries.is_empty() && state.background.is_empty() {
                state.held = true;
                state.streak = 0;
                return Ok(Grant {
                    scheduler: Arc::clone(self),
                });
            }
            if class == Class::Query && state.queries.len() >= self.max_waiting_queries {
                return Err(Refusal::QueueFull);
            }
            let id = state.next_id;
            state.next_id += 1;
            let granted = Arc::new(AtomicBool::new(false));
            let (tx, rx) = oneshot::channel();
            let waiter = Waiter {
                id,
                granted: Arc::clone(&granted),
                wake: tx,
            };
            match class {
                Class::Query => state.queries.push_back(waiter),
                Class::Background => state.background.push_back(waiter),
            }
            (id, granted, rx)
        };
        let mut waiting = Waiting {
            scheduler: self,
            id,
            class,
            granted,
            received: false,
        };
        let outcome = wake.await;
        waiting.received = true;
        match outcome {
            Ok(()) => Ok(Grant {
                scheduler: Arc::clone(self),
            }),
            Err(_) => Err(Refusal::Closed),
        }
    }

    /// The slot when it is free and nobody waits; a synchronous caller cannot queue.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn try_acquire(self: &Arc<Self>) -> Option<Grant> {
        let mut state = self.lock();
        if state.closed || state.held || !state.queries.is_empty() || !state.background.is_empty() {
            return None;
        }
        state.held = true;
        Some(Grant {
            scheduler: Arc::clone(self),
        })
    }

    /// Hands the slot to the next waiter that is still listening, or frees it.
    fn release(&self) {
        let mut state = self.lock();
        while let Some(waiter) = state.next() {
            waiter.granted.store(true, Ordering::Release);
            if waiter.wake.send(()).is_ok() {
                return;
            }
        }
        state.held = false;
        drop(state);
        self.idle.notify_waiters();
    }

    /// Refuses new waiters and removes every queued one; each queued `acquire` returns [`Refusal::Closed`]. A held slot stays held until its grant drops.
    pub(crate) fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.queries.clear();
        state.background.clear();
    }

    /// Resolves once no grant is held.
    pub(crate) async fn idle(&self) {
        loop {
            let notified = self.idle.notified();
            if !self.lock().held {
                return;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self) -> (usize, usize) {
        let state = self.lock();
        (state.queries.len(), state.background.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    /// Spawns one waiter of `class` that records `label` when granted and holds the slot until `release` is notified.
    fn waiter(
        scheduler: &Arc<Scheduler>,
        class: Class,
        label: &'static str,
        order: &Arc<StdMutex<Vec<&'static str>>>,
    ) -> tokio::task::JoinHandle<Result<(), Refusal>> {
        let scheduler = Arc::clone(scheduler);
        let order = Arc::clone(order);
        tokio::spawn(async move {
            let grant = scheduler.acquire(class).await?;
            order.lock().unwrap().push(label);
            tokio::task::yield_now().await;
            drop(grant);
            Ok(())
        })
    }

    async fn settle(scheduler: &Scheduler, queries: usize, background: usize) {
        for _ in 0..1000 {
            if scheduler.waiting() == (queries, background) {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "waiters never reached {queries} queries and {background} background: {:?}",
            scheduler.waiting()
        );
    }

    #[tokio::test]
    async fn queries_go_first_in_fifo_order_and_the_ninth_grant_serves_background() {
        let scheduler = Scheduler::new(4);
        let order = Arc::new(StdMutex::new(Vec::new()));
        let holder = scheduler.acquire(Class::Background).await.unwrap();
        let first_background = waiter(&scheduler, Class::Background, "b1", &order);
        let second_background = waiter(&scheduler, Class::Background, "b2", &order);
        settle(&scheduler, 0, 2).await;
        let mut queries = Vec::new();
        for label in ["q1", "q2", "q3", "q4"] {
            queries.push(waiter(&scheduler, Class::Query, label, &order));
            settle(&scheduler, queries.len(), 2).await;
        }
        // An active background holder does not let a fifth query wait.
        assert_eq!(
            scheduler.acquire(Class::Query).await.err(),
            Some(Refusal::QueueFull)
        );
        // Each granted query is replaced by another, so queries never stop arriving.
        let labels = [
            "q5", "q6", "q7", "q8", "q9", "q10", "q11", "q12", "q13", "q14",
        ];
        let mut refill = labels.iter();
        drop(holder);
        let mut more = Vec::new();
        while order.lock().unwrap().len() < 9 {
            let (waiting_queries, _) = scheduler.waiting();
            if waiting_queries < 4
                && let Some(label) = refill.next()
            {
                more.push(waiter(&scheduler, Class::Query, label, &order));
            }
            tokio::task::yield_now().await;
        }
        let granted = order.lock().unwrap()[..9].to_vec();
        assert_eq!(
            granted,
            ["q1", "q2", "q3", "q4", "q5", "q6", "q7", "q8", "b1"]
        );
        for task in queries.into_iter().chain(more) {
            task.await.unwrap().unwrap();
        }
        first_background.await.unwrap().unwrap();
        second_background.await.unwrap().unwrap();
        assert!(order.lock().unwrap().contains(&"b2"));
    }

    #[tokio::test]
    async fn a_cancelled_waiter_leaves_and_a_handed_but_untaken_grant_returns() {
        let scheduler = Scheduler::new(4);
        let holder = scheduler.acquire(Class::Query).await.unwrap();
        let abandoned = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move { scheduler.acquire(Class::Query).await.map(drop) })
        };
        settle(&scheduler, 1, 0).await;
        abandoned.abort();
        let _ = abandoned.await;
        assert_eq!(scheduler.waiting(), (0, 0));
        drop(holder);
        // The slot is free again: nobody held a grant the cancelled waiter took with it.
        let again = scheduler.try_acquire().expect("the slot was returned");

        // A waiter handed the slot in the same instant it is dropped returns the slot.
        let pending = scheduler.acquire(Class::Background);
        let mut pending = Box::pin(pending);
        assert!(
            futures_poll_once(pending.as_mut()).await.is_none(),
            "the slot is held, so the waiter queues"
        );
        drop(again);
        drop(pending);
        tokio::time::timeout(Duration::from_secs(1), scheduler.idle())
            .await
            .expect("the handed grant was returned");
        assert!(scheduler.try_acquire().is_some());
    }

    #[tokio::test]
    async fn close_refuses_queued_and_new_waiters_and_idle_waits_for_the_holder() {
        let scheduler = Scheduler::new(4);
        let holder = scheduler.acquire(Class::Background).await.unwrap();
        let queued = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move { scheduler.acquire(Class::Query).await.map(drop) })
        };
        settle(&scheduler, 1, 0).await;
        scheduler.close();
        assert_eq!(queued.await.unwrap(), Err(Refusal::Closed));
        assert_eq!(
            scheduler.acquire(Class::Background).await.err(),
            Some(Refusal::Closed)
        );
        assert!(scheduler.try_acquire().is_none());
        let idle = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move { scheduler.idle().await })
        };
        tokio::task::yield_now().await;
        assert!(!idle.is_finished(), "the holder still owns the slot");
        drop(holder);
        tokio::time::timeout(Duration::from_secs(1), idle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn zero_waiting_queries_refuses_every_query_behind_a_holder() {
        let scheduler = Scheduler::new(0);
        let holder = scheduler.acquire(Class::Query).await.unwrap();
        assert_eq!(
            scheduler.acquire(Class::Query).await.err(),
            Some(Refusal::QueueFull)
        );
        drop(holder);
        assert!(scheduler.acquire(Class::Query).await.is_ok());
    }

    /// Polls `future` once; `None` when it is still pending.
    async fn futures_poll_once<F: std::future::Future + Unpin>(future: F) -> Option<F::Output> {
        let mut future = future;
        std::future::poll_fn(|cx| {
            std::task::Poll::Ready(match std::pin::Pin::new(&mut future).poll(cx) {
                std::task::Poll::Ready(output) => Some(output),
                std::task::Poll::Pending => None,
            })
        })
        .await
    }
}
