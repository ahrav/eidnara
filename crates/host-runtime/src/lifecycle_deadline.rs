//! Lifecycle callback deadlines charge elapsed time only for steps whose wakes arrive at most `HEARTBEAT` late.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

/// `HEARTBEAT` caps each sleep and sets the maximum wake lateness charged to `deadline`.
const HEARTBEAT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(crate) struct Expired;

/// Returns `future`'s output, or `Expired` once the charged steps cover `deadline`.
///
/// Each step sleeps `min(HEARTBEAT, remaining)`. A step whose wake arrives more than `HEARTBEAT`
/// late preserves the remaining deadline budget. Every other step charges its measured length.
pub(crate) async fn within<F: Future>(deadline: Duration, future: F) -> Result<F::Output, Expired> {
    tokio::pin!(future);
    let mut remaining = deadline;
    loop {
        let step = remaining.min(HEARTBEAT);
        let started = Instant::now();
        tokio::select! {
            biased;
            output = &mut future => return Ok(output),
            () = tokio::time::sleep(step) => {}
        }
        let elapsed = started.elapsed();
        if elapsed.saturating_sub(step) > HEARTBEAT {
            continue;
        }
        remaining = remaining.saturating_sub(elapsed);
        if remaining.is_zero() {
            return Err(Expired);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEADLINE: Duration = Duration::from_secs(30);

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

    async fn jump(by: Duration) {
        tokio::time::advance(by).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
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
    async fn a_zero_deadline_polls_the_future_once() {
        assert_eq!(within(Duration::ZERO, async { 3 }).await.expect("ready"), 3);
        assert!(
            within(Duration::ZERO, std::future::pending::<()>())
                .await
                .is_err()
        );
    }
}
