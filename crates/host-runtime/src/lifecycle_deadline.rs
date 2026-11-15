//! Lifecycle callback deadlines charge elapsed time only for steps whose wakes arrive at most `HEARTBEAT` late.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::Instant;

/// `HEARTBEAT` caps each sleep and sets the maximum wake lateness charged to `deadline`.
const HEARTBEAT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(crate) struct Expired;

#[derive(Clone, Default)]
pub(crate) struct PollTime(Arc<AtomicU64>);

impl PollTime {
    pub(crate) async fn timing<F: Future>(self, future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            let entered = Instant::now();
            let poll = future.as_mut().poll(cx);
            let nanos = u64::try_from(entered.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.0.fetch_add(nanos, Ordering::Relaxed);
            poll
        })
        .await
    }

    fn total(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::Relaxed))
    }
}

pub(crate) async fn within<F: Future>(deadline: Duration, future: F) -> Result<F::Output, Expired> {
    let polls = PollTime::default();
    within_timed(deadline, polls.clone().timing(future), &polls).await
}

/// Returns `future`'s output, or `Expired` once the charged steps cover `deadline`.
///
/// Each step waits `min(HEARTBEAT, remaining)` and charges its measured length. A step whose wake
/// arrives more than `HEARTBEAT` late, after subtracting the time `polls` gained, charges only
/// that poll time.
pub(crate) async fn within_timed<F: Future>(
    deadline: Duration,
    future: F,
    polls: &PollTime,
) -> Result<F::Output, Expired> {
    tokio::pin!(future);
    let mut remaining = deadline;
    loop {
        let step = remaining.min(HEARTBEAT);
        let started = Instant::now();
        let polled_before = polls.total();
        if let Ok(output) = tokio::time::timeout(step, &mut future).await {
            return Ok(output);
        }
        let elapsed = started.elapsed();
        let polled = polls.total().saturating_sub(polled_before).min(elapsed);
        let late = elapsed.saturating_sub(step).saturating_sub(polled);
        let charged = if late > HEARTBEAT { polled } else { elapsed };
        remaining = remaining.saturating_sub(charged);
        if remaining.is_zero() {
            return Err(Expired);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(30);

    async fn jump(by: Duration) {
        tokio::time::advance(by).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }

    fn polling_for(per_poll: Duration, polls: usize) -> impl Future<Output = ()> {
        let mut left = polls;
        std::future::poll_fn(move |cx| {
            left = left.checked_sub(1).expect("the wait expires first");
            let mut advance = std::pin::pin!(tokio::time::advance(per_poll));
            let _ = advance.as_mut().poll(cx);
            cx.waker().wake_by_ref();
            Poll::Pending
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_pending_future_expires_at_the_deadline_when_every_heartbeat_is_on_time() {
        let started = Instant::now();
        let result = within(DEADLINE, std::future::pending::<()>()).await;
        assert!(result.is_err());
        assert_eq!(started.elapsed(), DEADLINE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_future_that_completes_inside_the_deadline_returns_its_output() {
        let result = within(DEADLINE, async {
            tokio::time::sleep(DEADLINE - Duration::from_millis(1)).await;
            7
        })
        .await;
        assert_eq!(result.expect("completed inside the deadline"), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn a_clock_jump_keeps_the_budget_the_wait_had_before_it() {
        let wait = tokio::spawn(within(DEADLINE, std::future::pending::<()>()));
        tokio::time::sleep(Duration::from_millis(10_500)).await;
        tokio::time::advance(DEADLINE * 4).await;
        let resumed = Instant::now();
        assert!(!wait.is_finished(), "the jump charges nothing");

        assert!(wait.await.expect("wait task").is_err());
        assert_eq!(resumed.elapsed(), DEADLINE - Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn a_future_that_completes_after_a_clock_jump_returns_its_output() {
        let (release, released) = tokio::sync::oneshot::channel::<u8>();
        let wait = tokio::spawn(within(DEADLINE, released));
        tokio::task::yield_now().await;
        tokio::time::advance(DEADLINE * 4).await;
        release.send(9).expect("the wait holds the receiver");
        let output = wait
            .await
            .expect("wait task")
            .expect("the jump is not charged");
        assert_eq!(output, Ok(9));
    }

    #[tokio::test(start_paused = true)]
    async fn a_wake_one_heartbeat_late_is_charged_in_full() {
        let wait = tokio::spawn(within(DEADLINE, std::future::pending::<()>()));
        tokio::task::yield_now().await;
        for _ in 0..15 {
            assert!(!wait.is_finished());
            jump(HEARTBEAT * 2).await;
        }
        assert!(wait.await.expect("wait task").is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_wake_over_one_heartbeat_late_charges_nothing() {
        let wait = tokio::spawn(within(DEADLINE, std::future::pending::<()>()));
        tokio::task::yield_now().await;
        for _ in 0..100 {
            jump(HEARTBEAT * 2 + Duration::from_millis(1)).await;
        }
        assert!(!wait.is_finished());
        wait.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_future_whose_polls_outlast_a_heartbeat_is_charged_its_poll_time() {
        let started = Instant::now();
        let result = within(DEADLINE, polling_for(HEARTBEAT * 3, 11)).await;
        assert!(result.is_err());
        assert_eq!(started.elapsed(), DEADLINE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_join_wait_charges_the_poll_time_of_the_task_it_joins() {
        let polls = PollTime::default();
        let task = tokio::spawn(polls.clone().timing(polling_for(HEARTBEAT * 3, 200)));
        let result = within_timed(DEADLINE, task, &polls).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn a_future_that_spends_its_poll_budget_still_expires() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
        let spinning = async {
            loop {
                sender.send(()).expect("receiver is held");
                receiver.recv().await;
            }
        };
        let expiring = within(Duration::from_millis(50), spinning);
        let result = tokio::time::timeout(Duration::from_secs(10), expiring).await;
        assert!(matches!(result, Ok(Err(Expired))));
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_deadline_polls_the_future_once() {
        assert_eq!(within(Duration::ZERO, async { 3 }).await.expect("ready"), 3);
        assert!(
            within(Duration::ZERO, std::future::pending::<()>())
                .await
                .is_err()
        );
    }
}
