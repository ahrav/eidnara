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
            // `granted` is set only for a delivered wake, under the lock the dropping waiter also takes, so a waiter whose receiver was already gone is never told it holds the slot.
            if waiter.wake.send(()).is_ok() {
                waiter.granted.store(true, Ordering::Release);
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
        for attempt in 0..20_000 {
            if scheduler.waiting() == (queries, background) {
                return;
            }
            // Yield first; under load, give other workers wall time.
            if attempt < 1000 {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
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
        let refused = tokio::time::timeout(Duration::from_secs(1), scheduler.acquire(Class::Query))
            .await
            .expect("a full queue refuses at once");
        assert_eq!(refused.err(), Some(Refusal::QueueFull));
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

    /// Waiters dropped while the holder releases never leave two grants live: each released slot reaches exactly one listening waiter or goes idle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn waiters_dropped_during_release_never_double_grant() {
        use std::sync::atomic::AtomicUsize;
        let scheduler = Scheduler::new(4);
        let active = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        for round in 0..500 {
            let holder = scheduler.acquire(Class::Query).await.unwrap();
            let mut tasks = Vec::new();
            for class in [Class::Query, Class::Background, Class::Query] {
                let scheduler = Arc::clone(&scheduler);
                let active = Arc::clone(&active);
                let most = Arc::clone(&most);
                tasks.push(tokio::spawn(async move {
                    struct Active(Arc<AtomicUsize>);
                    impl Drop for Active {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    if let Ok(grant) = scheduler.acquire(class).await {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        // Declared after the grant, so an abort drops it first and the count falls before the slot is released.
                        let _active = Active(Arc::clone(&active));
                        tokio::task::yield_now().await;
                        drop(_active);
                        drop(grant);
                    }
                }));
            }
            settle(&scheduler, 2, 1).await;
            let release = tokio::spawn(async move { drop(holder) });
            // Abort one waiter in the same instant the holder releases.
            tasks[round % 3].abort();
            release.await.unwrap();
            for task in tasks {
                let _ = task.await;
            }
            tokio::time::timeout(Duration::from_secs(1), scheduler.idle())
                .await
                .expect("every grant was released");
            assert_eq!(scheduler.waiting(), (0, 0));
        }
        assert_eq!(most.load(Ordering::SeqCst), 1);
    }

    /// A waiter whose receiver is already gone when the slot reaches it is never marked granted, so its own drop cannot release the slot a second time; the slot moves on to the next listening waiter.
    #[tokio::test]
    async fn a_waiter_whose_receiver_is_gone_is_never_marked_granted() {
        let scheduler = Scheduler::new(4);
        let holder = scheduler.acquire(Class::Query).await.unwrap();
        let gone = Arc::new(AtomicBool::new(false));
        {
            let (wake, receiver) = oneshot::channel();
            drop(receiver);
            let mut state = scheduler.lock();
            let id = state.next_id;
            state.next_id += 1;
            state.queries.push_back(Waiter {
                id,
                granted: Arc::clone(&gone),
                wake,
            });
        }
        let listening = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move { scheduler.acquire(Class::Query).await.map(drop) })
        };
        settle(&scheduler, 2, 0).await;
        drop(holder);
        assert_eq!(listening.await.unwrap(), Ok(()));
        assert!(
            !gone.load(Ordering::SeqCst),
            "an undelivered wake grants nothing"
        );
        assert!(
            scheduler.try_acquire().is_some(),
            "the slot is free once the listener drops its grant"
        );
    }

    /// The streak counts only query grants made while background work waits, so queries granted before a background text arrives do not shorten its wait.
    #[test]
    fn the_streak_counts_only_while_background_waits() {
        let waiter = |state: &mut State| {
            let (wake, _) = oneshot::channel();
            let id = state.next_id;
            state.next_id += 1;
            Waiter {
                id,
                granted: Arc::new(AtomicBool::new(false)),
                wake,
            }
        };
        let mut state = State::default();
        for _ in 0..12 {
            let query = waiter(&mut state);
            state.queries.push_back(query);
        }
        for _ in 0..QUERY_STREAK {
            assert!(state.next().is_some());
        }
        assert_eq!(state.streak, 0, "no background waited");
        let background = waiter(&mut state);
        let background_id = background.id;
        state.background.push_back(background);
        let next = state.next().unwrap();
        assert_ne!(
            next.id, background_id,
            "a fresh streak serves the query first"
        );
        assert_eq!(state.streak, 1);
    }

    #[tokio::test]
    async fn zero_waiting_queries_refuses_every_query_behind_a_holder() {
        let scheduler = Scheduler::new(0);
        let holder = scheduler.acquire(Class::Query).await.unwrap();
        let refused = tokio::time::timeout(Duration::from_secs(1), scheduler.acquire(Class::Query))
            .await
            .expect("a full queue refuses at once");
        assert_eq!(refused.err(), Some(Refusal::QueueFull));
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
