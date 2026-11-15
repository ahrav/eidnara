//! A host suspended past a lifecycle callback deadline keeps serving after resuming; a hung lifecycle callback is fatal.

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
            let entered = thread_cpu_time();
            let poll = future.as_mut().poll(cx);
            let spent = thread_cpu_time().saturating_sub(entered);
            let nanos = u64::try_from(spent.as_nanos()).unwrap_or(u64::MAX);
            self.0.fetch_add(nanos, Ordering::Relaxed);
            poll
        })
        .await
    }

    fn total(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::Relaxed))
    }
}

fn thread_cpu_time() -> Duration {
    #[cfg(test)]
    if let Some(spent) = test_cpu::spent() {
        return spent;
    }
    let now = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    Duration::new(
        u64::try_from(now.tv_sec).unwrap_or(0),
        u32::try_from(now.tv_nsec).unwrap_or(0),
    )
}

#[cfg(test)]
pub(crate) mod test_cpu {
    use std::cell::Cell;
    use std::time::Duration;

    thread_local! {
        static SPENT: Cell<Option<Duration>> = const { Cell::new(None) };
    }

    pub(crate) fn spent() -> Option<Duration> {
        SPENT.get()
    }

    pub(crate) fn spend(cpu: Duration) {
        SPENT.set(Some(SPENT.get().unwrap_or_default() + cpu));
    }
}

pub(crate) async fn within<F: Future>(deadline: Duration, future: F) -> Result<F::Output, Expired> {
    let polls = PollTime::default();
    within_timed(deadline, polls.clone().timing(future), &polls).await
}

/// Returns `future`'s output, or `Expired` once the charged steps cover `deadline`.
///
/// Each step waits `min(HEARTBEAT, remaining)` and charges its measured length. A step whose wake
/// arrives more than `HEARTBEAT` late, after subtracting the thread CPU time `polls` gained,
/// charges that CPU time, and at least its `step` when the step before it was late too.
pub(crate) async fn within_timed<F: Future>(
    deadline: Duration,
    future: F,
    polls: &PollTime,
) -> Result<F::Output, Expired> {
    tokio::pin!(future);
    let mut remaining = deadline;
    let mut previous_late = false;
    loop {
        let step = remaining.min(HEARTBEAT);
        let started = Instant::now();
        let polled_before = polls.total();
        if let Ok(output) = tokio::time::timeout(step, &mut future).await {
            return Ok(output);
        }
        let elapsed = started.elapsed();
        let polled = polls.total().saturating_sub(polled_before);
        let late = elapsed.saturating_sub(step).saturating_sub(polled) > HEARTBEAT;
        let late_charge = polled.min(step + HEARTBEAT);
        let charged = match (late, previous_late) {
            (false, _) => elapsed,
            (true, false) => late_charge,
            (true, true) => late_charge.max(step),
        };
        previous_late = late;
        remaining = remaining.saturating_sub(charged);
        if remaining.is_zero() {
            return Err(Expired);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::task::Poll;

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(30);

    async fn jump(by: Duration) {
        tokio::time::advance(by).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }

    fn polling(wall: Duration, cpu: Duration, polls: Arc<AtomicUsize>) -> impl Future<Output = ()> {
        std::future::poll_fn(move |cx| {
            assert!(
                polls.fetch_add(1, Ordering::SeqCst) < 1_000,
                "the wait expires first"
            );
            test_cpu::spend(cpu);
            let mut advance = std::pin::pin!(tokio::time::advance(wall));
            let _ = advance.as_mut().poll(cx);
            cx.waker().wake_by_ref();
            Poll::Pending
        })
    }

    async fn wall_time_until_expiry(wall: Duration, cpu: Duration) -> Duration {
        test_cpu::spend(Duration::ZERO);
        let started = Instant::now();
        let polls = Arc::new(AtomicUsize::new(0));
        let result = within(DEADLINE, polling(wall, cpu, polls)).await;
        assert!(result.is_err());
        started.elapsed()
    }

    #[tokio::test(start_paused = true)]
    async fn a_pending_future_expires_at_the_deadline_when_every_heartbeat_is_on_time() {
        test_cpu::spend(Duration::ZERO);
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
        test_cpu::spend(Duration::ZERO);
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
        test_cpu::spend(Duration::ZERO);
        let wait = tokio::spawn(within(DEADLINE, std::future::pending::<()>()));
        tokio::task::yield_now().await;
        for _ in 0..15 {
            assert!(!wait.is_finished());
            jump(HEARTBEAT * 2).await;
        }
        assert!(wait.is_finished());
        assert!(wait.await.expect("wait task").is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn consecutive_late_wakes_charge_one_step_each_after_the_first() {
        test_cpu::spend(Duration::ZERO);
        let wait = tokio::spawn(within(DEADLINE, std::future::pending::<()>()));
        tokio::task::yield_now().await;
        for _ in 0..31 {
            assert!(!wait.is_finished());
            jump(HEARTBEAT * 2 + Duration::from_millis(1)).await;
        }
        assert!(wait.is_finished());
        assert!(wait.await.expect("wait task").is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_wake_between_on_time_wakes_charges_nothing() {
        test_cpu::spend(Duration::ZERO);
        let wait = tokio::spawn(within(DEADLINE, std::future::pending::<()>()));
        tokio::task::yield_now().await;
        for _ in 0..29 {
            jump(DEADLINE * 2).await;
            jump(HEARTBEAT).await;
        }
        assert!(!wait.is_finished());
        jump(HEARTBEAT).await;
        assert!(wait.is_finished());
    }

    #[tokio::test(start_paused = true)]
    async fn a_future_whose_polls_outlast_a_heartbeat_is_charged_its_elapsed_time() {
        let waited = wall_time_until_expiry(HEARTBEAT * 3, HEARTBEAT * 3).await;
        assert_eq!(waited, DEADLINE);
    }

    #[tokio::test(start_paused = true)]
    async fn poll_cpu_time_counts_against_lateness() {
        let waited = wall_time_until_expiry(HEARTBEAT * 3 / 2, HEARTBEAT).await;
        assert_eq!(waited, DEADLINE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_step_charges_the_cpu_time_of_its_polls() {
        let waited = wall_time_until_expiry(HEARTBEAT * 2, HEARTBEAT * 3 / 4).await;
        assert_eq!(waited, HEARTBEAT * 80);
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_step_charges_at_most_two_heartbeats_of_poll_cpu_time() {
        let waited = wall_time_until_expiry(HEARTBEAT * 10, HEARTBEAT * 5 / 2).await;
        assert_eq!(waited, HEARTBEAT * 300);
    }

    #[tokio::test(start_paused = true)]
    async fn a_join_wait_charges_the_poll_time_of_the_task_it_joins() {
        test_cpu::spend(Duration::ZERO);
        let polls = PollTime::default();
        let count = Arc::new(AtomicUsize::new(0));
        let spinning = polling(HEARTBEAT * 3, HEARTBEAT * 3, Arc::clone(&count));
        let task = tokio::spawn(polls.clone().timing(spinning));
        let result = within_timed(DEADLINE, task, &polls).await;
        assert!(result.is_err());
        assert!(count.load(Ordering::SeqCst) < 100);
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

    #[tokio::test]
    async fn a_poll_that_burns_cpu_is_charged_from_the_thread_clock() {
        let polls = PollTime::default();
        let burning = polls.clone().timing(std::future::poll_fn(|_| {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_millis(200) {
                std::hint::spin_loop();
            }
            Poll::Ready(())
        }));
        burning.await;
        assert!(
            polls.total() >= Duration::from_millis(20),
            "{:?}",
            polls.total()
        );

        let sleeping = PollTime::default();
        sleeping
            .clone()
            .timing(std::future::poll_fn(|_| {
                std::thread::sleep(Duration::from_millis(50));
                Poll::Ready(())
            }))
            .await;
        assert!(
            sleeping.total() < Duration::from_millis(25),
            "{:?}",
            sleeping.total()
        );
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
