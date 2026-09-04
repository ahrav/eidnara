//! This module redacts panic diagnostics while handler callbacks execute.

use std::cell::Cell;
use std::future::Future;
use std::sync::Once;

const REDACTED_DIAGNOSTIC: &str = "eidnara-host handler callback panicked (details redacted)";

static INSTALL_HOOK: Once = Once::new();

thread_local! {
    static CALLBACK_POLL_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct CallbackPollGuard;

impl CallbackPollGuard {
    fn enter() -> Self {
        CALLBACK_POLL_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for CallbackPollGuard {
    fn drop(&mut self) {
        CALLBACK_POLL_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn callback_is_polling() -> bool {
    CALLBACK_POLL_DEPTH
        .try_with(|depth| depth.get() != 0)
        .unwrap_or(false)
}

pub fn install() {
    INSTALL_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if callback_is_polling() {
                // `eprintln!` panics when stderr is closed, and a panic inside the hook aborts
                // the process, so the write failure is ignored instead.
                use std::io::Write;
                let _ = writeln!(std::io::stderr().lock(), "{REDACTED_DIAGNOSTIC}");
            } else {
                previous(info);
            }
        }));
    });
}

pub fn redact_sync<T>(callback: impl FnOnce() -> T) -> T {
    let _guard = CallbackPollGuard::enter();
    callback()
}

/// A callback future that returns `Poll::Pending` cannot suppress an unrelated task's panic on the same worker.
/// The callback future is also dropped under the guard, so a destructor panic during
/// cancellation (a host deadline dropping the future while pending) is redacted too.
pub async fn redact<F: Future>(future: F) -> F::Output {
    let mut future = RedactedOnDrop(Some(Box::pin(future)));
    std::future::poll_fn(|cx| {
        let _guard = CallbackPollGuard::enter();
        future
            .0
            .as_mut()
            .expect("callback future is present until redact returns")
            .as_mut()
            .poll(cx)
    })
    .await
}

struct RedactedOnDrop<F>(Option<std::pin::Pin<Box<F>>>);

impl<F> Drop for RedactedOnDrop<F> {
    fn drop(&mut self) {
        let _guard = CallbackPollGuard::enter();
        drop(self.0.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// `RecordsPollingOnDrop` writes whether the redaction guard was active when it was dropped.
    struct RecordsPollingOnDrop(Arc<AtomicBool>);

    impl Drop for RecordsPollingOnDrop {
        fn drop(&mut self) {
            self.0.store(callback_is_polling(), Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn a_callback_future_dropped_while_pending_is_still_inside_the_boundary() {
        let seen_polling = Arc::new(AtomicBool::new(false));
        let state = RecordsPollingOnDrop(seen_polling.clone());
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            redact(async move {
                let _state = state;
                std::future::pending::<()>().await;
            }),
        )
        .await;
        assert!(cancelled.is_err(), "only the timeout can complete");
        assert!(
            seen_polling.load(Ordering::Relaxed),
            "the destructor ran outside the redaction guard"
        );
        assert!(
            !callback_is_polling(),
            "the guard is released after the drop"
        );
    }

    #[tokio::test]
    async fn a_completed_callback_future_is_dropped_inside_the_boundary() {
        let seen_polling = Arc::new(AtomicBool::new(false));
        let state = RecordsPollingOnDrop(seen_polling.clone());
        redact(async move {
            // `state` lives until the future is dropped, which happens after the last poll.
            let _state = &state;
        })
        .await;
        assert!(seen_polling.load(Ordering::Relaxed));
        assert!(!callback_is_polling());
    }
}
