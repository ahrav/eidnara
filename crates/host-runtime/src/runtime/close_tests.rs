use super::*;
use crate::connection::{PendingEntry, PendingKey};
use crate::dispatch::{Settlement, Terminal, settle, settle_route_work};
use crate::frame_channel::{SenderQueue, frame_sender};
use crate::handler::{
    BindOutcome, RequestCtx, RequestOutcome, RouteClass, RouteHandle, RouteIdentity, RouteTarget,
};
use crate::routing::CloseDecision;
use crate::wire::FrameId;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct UnusedHandler {
    route_gones: AtomicUsize,
    route_gones_started: AtomicUsize,
    hold_route_gone: AtomicBool,
    route_gone_release: tokio::sync::Notify,
}
impl HostHandler for UnusedHandler {
    fn manifests(&self) -> Vec<crate::ManifestSnapshot> {
        Vec::new()
    }
    async fn initialize(&self, _: crate::HostInit) -> Result<(), crate::InitError> {
        Ok(())
    }
    async fn bind(&self, _: RouteHandle, _: RouteTarget, _: RouteIdentity) -> BindOutcome {
        unreachable!()
    }
    async fn handle(&self, _: RequestCtx) -> RequestOutcome {
        unreachable!()
    }
    async fn route_gone(&self, _: RouteHandle) {
        self.route_gones_started.fetch_add(1, Ordering::SeqCst);
        while self.hold_route_gone.load(Ordering::SeqCst) {
            self.route_gone_release.notified().await;
        }
        self.route_gones.fetch_add(1, Ordering::SeqCst);
    }
    async fn health(&self) -> HealthReport {
        HealthReport::ok()
    }
    async fn shutdown(&self) {}
}

fn shared() -> Arc<HostShared<UnusedHandler>> {
    Arc::new(HostShared {
        handler: Arc::new(UnusedHandler::default()),
        limits: HostLimits::default(),
        timing: HostTiming {
            route_close_budget: Duration::from_millis(100),
            ..HostTiming::default()
        },
        liveness: None,
        targets: crate::control::TargetIndex::new(Vec::new()),
        catalog: crate::control::CatalogCache::new_bounded(&[], 4096).unwrap(),
        health_snapshot: RwLock::new(HealthReport::ok()),
        ring: Arc::new(crate::ring_transport::RingTransport::for_ring_profile(
            crate::ring_transport::process_limits(1).unwrap(),
        )),
        registry: RouteRegistry::new(4),
        ingress_budget: ByteBudget::new(4096),
        scratch_budget: ByteBudget::new(4096),
        egress_budget: ByteBudget::new(4096),
        terminal_budget: ByteBudget::new(1 << 20),
        pending_permits: Arc::new(Semaphore::new(4)),
        task_permits: Arc::new(Semaphore::new(4)),
        reserved_pending_permits: Arc::new(Semaphore::new(0)),
        reserved_task_permits: Arc::new(Semaphore::new(0)),
        handshake_permits: Arc::new(Semaphore::new(1)),
        connection_permits: Arc::new(Semaphore::new(1)),
        tracker: TaskTracker::new(),
        deferred_closes: TaskTracker::new(),
        deferred_trackers: Mutex::new(Vec::new()),
        abort_handles: Mutex::new(AbortRegistry::new()),
        shutdown_callback_ran: AtomicBool::new(false),
        shutdown: CancellationToken::new(),
        shutdown_latch: Arc::new(crate::lifecycle::ShutdownLatch::new()),
        draining: AtomicBool::new(false),
        fatal: FatalCell::new(),
        gen_counter: AtomicU64::new(1),
        connections: Mutex::new(HashMap::new()),
        auth_key: ConnectionKey([0; 32]),
        daemon_id: [0; 16],
        daemon_ver: "test".into(),
    })
}

struct Aborted(Option<tokio::sync::oneshot::Sender<()>>);
impl Drop for Aborted {
    fn drop(&mut self) {
        let _ = self.0.take().unwrap().send(());
    }
}

struct CloseFixture {
    shared: Arc<HostShared<UnusedHandler>>,
    generation: Arc<GenerationCore>,
    queue: SenderQueue,
    route: RouteHandle,
    key: PendingKey,
    cancelled: CancellationToken,
    settlement: Arc<Settlement>,
    decision: CloseDecision,
    aborted: tokio::sync::oneshot::Receiver<()>,
    physical_work: tokio_util::task::task_tracker::TaskTrackerToken,
}

fn fixture() -> CloseFixture {
    fixture_on(shared(), 1)
}

/// A fixture whose route lives on `shared`, on a generation numbered `generation_id`.
fn fixture_on(shared: Arc<HostShared<UnusedHandler>>, generation_id: u64) -> CloseFixture {
    let token = CancellationToken::new();
    let (writer, queue) = frame_sender(8, token.clone(), Duration::from_secs(30));
    let generation = Arc::new(GenerationCore {
        id: generation_id,
        token,
        read_cancel: CancellationToken::new(),
        read_tasks: TaskTracker::new(),
        shutdown_complete: CancellationToken::new(),
        writer,
        pending: Mutex::new(HashMap::new()),
        pings: Mutex::new(HashMap::new()),
        busy_rejects: Arc::new(Semaphore::new(4)),
        terminal_credits: Arc::new(tokio::sync::Semaphore::new(
            crate::config::TERMINAL_CREDITS_PER_CONNECTION,
        )),
        next_ping_corr: AtomicU64::new(1),
    });
    let route = shared
        .registry
        .reserve(&generation, RouteClass::General)
        .unwrap();
    shared.registry.install_bound(route);
    let (tracker, _, cancelled) = shared.registry.route_tracker(route, generation_id).unwrap();
    let key = (route.channel, route.epoch, 1);
    let settlement = Settlement::with_credit(None);
    generation.pending.lock().unwrap().insert(
        key,
        PendingEntry {
            cancel: cancelled.clone(),
            settlement: settlement.clone(),
            _pending_permit: shared.pending_permits.clone().try_acquire_owned().unwrap(),
        },
    );
    let (aborted_tx, aborted) = tokio::sync::oneshot::channel();
    let guard = Aborted(Some(aborted_tx));
    let outer = tracker.spawn(async move {
        let _guard = guard;
        std::future::pending::<()>().await;
    });
    shared
        .registry
        .register_dispatch(route, generation_id, outer.abort_handle())
        .unwrap();
    let physical_work = tracker.token();
    let decision = shared.registry.begin_close(route);
    CloseFixture {
        shared,
        generation,
        queue,
        route,
        key,
        cancelled,
        settlement,
        decision,
        aborted,
        physical_work,
    }
}

#[tokio::test(start_paused = true)]
async fn post_abort_completion_settles_once_without_wall_clock_sleeps() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        route,
        settlement,
        decision,
        aborted,
        physical_work,
        ..
    } = fixture();
    let close = tokio::spawn(async move { settle_route_work(&shared, route, decision).await });
    aborted.await.unwrap();
    assert!(!settlement.is_settled());
    assert!(queue.try_recv().is_err());
    drop(physical_work);
    assert!(close.await.unwrap());
    assert!(settlement.is_settled());
    let terminal = queue.try_recv().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&terminal.bytes[crate::wire::HEADER_LEN..])
            .unwrap()["code"],
        "cancelled"
    );
    assert!(queue.try_recv().is_err());
    assert!(!generation.token.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn live_work_after_both_close_windows_defers_route_gone_until_it_stops() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        route,
        settlement,
        decision,
        aborted,
        physical_work,
        ..
    } = fixture();
    let started = Instant::now();
    let close_shared = shared.clone();
    let close =
        tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
    aborted.await.unwrap();
    assert!(
        !close.await.unwrap(),
        "held physical work defers route-gone past the close owner"
    );
    assert_eq!(Instant::now() - started, Duration::from_millis(200));
    assert!(
        !shared.shutdown.is_cancelled() && shared.fatal.take().is_none(),
        "work outliving the close budget fails its route, not the host"
    );
    assert!(queue.try_recv().is_err());
    assert!(!generation.pending.lock().unwrap().is_empty());
    assert_eq!(
        shared.registry.live_count(),
        1,
        "the held route keeps its channel"
    );

    drop(physical_work);
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("deferred closes finish once their work stops");
    assert!(settlement.is_settled());
    let terminal = queue.try_recv().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&terminal.bytes[crate::wire::HEADER_LEN..])
            .unwrap()["code"],
        "cancelled"
    );
    assert!(generation.pending.lock().unwrap().is_empty());
    assert_eq!(
        shared.registry.live_count(),
        0,
        "route-gone released the channel"
    );
    assert!(!shared.shutdown.is_cancelled());
}

/// Forced shutdown's drain and the deferred close both own a route whose work outlived the close
/// budget; when the work stops, route-gone runs once, the channel stays held until it returns,
/// and the deferred close still delivers the request's `cancelled` terminal.
#[tokio::test(start_paused = true)]
async fn a_deferred_close_and_the_forced_drain_run_route_gone_once() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        route,
        settlement,
        decision,
        aborted,
        physical_work,
        ..
    } = fixture();
    shared.handler.hold_route_gone.store(true, Ordering::SeqCst);
    let close_shared = shared.clone();
    let close =
        tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
    aborted.await.unwrap();
    assert!(!close.await.unwrap());
    let forced_shared = shared.clone();
    let forced =
        tokio::spawn(async move { crate::dispatch::force_close_all_routes(&forced_shared).await });
    // The paused clock auto-advances once every task is idle, so this sleep returns after the
    // forced drain has parked on the route tracker beside the deferred close.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !forced.is_finished(),
        "the forced drain waits on the route tracker"
    );
    assert_eq!(shared.handler.route_gones_started.load(Ordering::SeqCst), 0);

    drop(physical_work);
    tokio::time::timeout(Duration::from_secs(1), async {
        while shared.handler.route_gones_started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("route-gone starts once the work stops");
    // One owner holds the claim inside route-gone; the other returns once it loses the claim,
    // ending either the forced drain or the deferred close.
    tokio::time::timeout(Duration::from_secs(1), async {
        while !forced.is_finished() && !shared.deferred_closes.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the owner that lost the claim returns");
    assert_eq!(
        shared.registry.live_count(),
        1,
        "the owner that lost the route-gone claim keeps the channel while route-gone runs"
    );
    shared
        .handler
        .hold_route_gone
        .store(false, Ordering::SeqCst);
    shared.handler.route_gone_release.notify_one();
    forced.await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("deferred closes finish once their work stops");
    assert_eq!(
        shared.handler.route_gones_started.load(Ordering::SeqCst),
        1,
        "route-gone must run once across both owners"
    );
    assert_eq!(shared.handler.route_gones.load(Ordering::SeqCst), 1);
    assert_eq!(shared.registry.live_count(), 0);
    assert!(settlement.is_settled());
    let terminal = queue.try_recv().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&terminal.bytes[crate::wire::HEADER_LEN..])
            .unwrap()["code"],
        "cancelled"
    );
    assert!(generation.pending.lock().unwrap().is_empty());
    assert!(!generation.token.is_cancelled());
    assert!(shared.fatal.take().is_none());
}

/// Deferred closes trip the fatal latch once the route-tracker entries they wait on exceed
/// `MAX_DEFERRED_WORK`; at the limit the host keeps serving.
#[tokio::test(start_paused = true)]
async fn deferred_work_past_its_limit_trips_the_fatal_latch() {
    for (extra, fatal) in [
        (crate::dispatch::MAX_DEFERRED_WORK - 1, false),
        (crate::dispatch::MAX_DEFERRED_WORK, true),
    ] {
        let CloseFixture {
            shared,
            route,
            decision,
            aborted,
            physical_work,
            ..
        } = fixture();
        let CloseDecision::Owner { tracker, .. } = &decision else {
            panic!("the fixture hands close ownership to the caller");
        };
        let held: Vec<_> = (0..extra).map(|_| tracker.token()).collect();
        let close_shared = shared.clone();
        let close =
            tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
        aborted.await.unwrap();
        assert!(!close.await.unwrap());
        assert_eq!(
            shared.fatal.take().is_some(),
            fatal,
            "{} held entries against a limit of {}",
            extra + 1,
            crate::dispatch::MAX_DEFERRED_WORK
        );
        drop(held);
        drop(physical_work);
        tokio::time::timeout(Duration::from_secs(1), async {
            shared.tracker.close();
            shared.tracker.wait().await;
        })
        .await
        .expect("the deferred close finishes once its work stops");
        assert_eq!(shared.deferred_work(), 0);
    }
}

/// A dispatch task aborted inside `settle`, after it wins and before its terminal is queued,
/// leaves the request to its route close: the close queues one `cancelled` terminal carrying a
/// fresh terminal credit, and the generation keeps serving.
#[tokio::test(start_paused = true)]
async fn a_settle_aborted_before_queuing_is_settled_by_the_route_close() {
    settle_aborted_before_queuing(Credits::Available).await;
}

/// The retried terminal waits for a terminal credit under the post-abort deadline; with every
/// credit held until then, the close retires the generation at the deadline instead of queuing
/// an uncredited terminal.
#[tokio::test(start_paused = true)]
async fn a_retried_terminal_without_a_credit_retires_the_generation() {
    settle_aborted_before_queuing(Credits::HeldPastDeadline).await;
}

/// A credit that frees inside the post-abort window carries the retried terminal, and the
/// generation keeps serving.
#[tokio::test(start_paused = true)]
async fn a_retried_terminal_takes_a_credit_that_frees_inside_the_window() {
    settle_aborted_before_queuing(Credits::FreedInsideWindow).await;
}

#[derive(Clone, Copy, PartialEq)]
enum Credits {
    Available,
    HeldPastDeadline,
    FreedInsideWindow,
}

async fn settle_aborted_before_queuing(credits_state: Credits) {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        route,
        key,
        settlement,
        decision,
        physical_work,
        ..
    } = fixture();
    // The request carries a real terminal credit, as an admitted request does.
    let credits = generation.terminal_credits.clone();
    let before = credits.available_permits();
    settlement.reattach_credit(&generation).await.unwrap();
    assert_eq!(credits.available_permits(), before - 1);
    // Holding the whole terminal reserve parks the winner in its budget wait.
    let reserve = shared.terminal_budget.try_charge(1 << 20).unwrap();
    let mut winner = Box::pin(settle(
        &settlement,
        &shared.terminal_budget,
        &generation,
        route,
        key.2,
        Terminal::Error {
            code: "handler_error".to_owned(),
            message: "handler failed".to_owned(),
            retry_after_ms: None,
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(winner.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(settlement.is_settled());
    drop(winner);
    assert_eq!(
        credits.available_permits(),
        before,
        "the aborted winner's credit returned"
    );
    let held = (credits_state != Credits::Available).then(|| {
        let all = u32::try_from(credits.available_permits()).unwrap();
        credits.clone().try_acquire_many_owned(all).unwrap()
    });
    drop(reserve);
    let close_shared = shared.clone();
    let close =
        tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
    // The fixture's dispatch task never ends, so the close aborts it after one 100 ms window
    // and the post-abort window ends at 200 ms.
    let started = Instant::now();
    tokio::time::sleep(Duration::from_millis(150)).await;
    drop(physical_work);
    if credits_state == Credits::FreedInsideWindow {
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(held);
        assert!(close.await.unwrap());
        assert!(!generation.token.is_cancelled());
        let terminal = queue.try_recv().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&terminal.bytes[crate::wire::HEADER_LEN..])
                .unwrap()["code"],
            "cancelled"
        );
        assert_eq!(credits.available_permits(), before - 1);
        return;
    }
    assert!(close.await.unwrap());
    if credits_state == Credits::HeldPastDeadline {
        assert_eq!(
            Instant::now() - started,
            Duration::from_millis(200),
            "the retry waits until the post-abort deadline"
        );
        assert!(generation.token.is_cancelled());
        assert!(
            queue.try_recv().is_err(),
            "no uncredited terminal was queued"
        );
        drop(held);
        return;
    }
    assert!(!generation.token.is_cancelled());
    let terminal = queue.try_recv().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&terminal.bytes[crate::wire::HEADER_LEN..])
            .unwrap()["code"],
        "cancelled"
    );
    assert!(queue.try_recv().is_err(), "one terminal for the request");
    assert_eq!(
        credits.available_permits(),
        before - 1,
        "the retried terminal carries a credit until its block returns"
    );
    drop(terminal);
    assert_eq!(credits.available_permits(), before);
}

/// `MAX_DEFERRED_WORK` bounds the sum across deferred closes on one host, and a close that
/// finishes returns exactly its own share.
#[tokio::test(start_paused = true)]
async fn deferred_work_sums_across_deferred_closes() {
    let shared = shared();
    let half = crate::dispatch::MAX_DEFERRED_WORK / 2;
    let mut held = Vec::new();
    let mut work = Vec::new();
    for (generation_id, extra, fatal) in [(1, half - 1, false), (2, half, true)] {
        let CloseFixture {
            route,
            decision,
            aborted,
            physical_work,
            ..
        } = fixture_on(shared.clone(), generation_id);
        let CloseDecision::Owner { tracker, .. } = &decision else {
            panic!("the fixture hands close ownership to the caller");
        };
        held.push((0..extra).map(|_| tracker.token()).collect::<Vec<_>>());
        let close_shared = shared.clone();
        let close =
            tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
        aborted.await.unwrap();
        assert!(!close.await.unwrap());
        assert_eq!(shared.fatal.take().is_some(), fatal);
        work.push(physical_work);
    }
    assert_eq!(
        shared.deferred_work(),
        crate::dispatch::MAX_DEFERRED_WORK + 1
    );
    drop(held.remove(0));
    drop(work.remove(0));
    tokio::time::timeout(Duration::from_secs(1), async {
        while shared.deferred_work() != half + 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the released close returns exactly its share");
    drop(held);
    drop(work);
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("deferred closes finish once their work stops");
    assert_eq!(shared.deferred_work(), 0);
}

/// The limit counts live entries: entries a deferred route finished before a later deferral do
/// not count against it.
#[tokio::test(start_paused = true)]
async fn deferred_work_counts_only_entries_still_held() {
    let shared = shared();
    let mut kept = Vec::new();
    for (generation_id, extra, release_early) in [(1, 40, 39), (2, 30, 0)] {
        let CloseFixture {
            route,
            decision,
            aborted,
            physical_work,
            ..
        } = fixture_on(shared.clone(), generation_id);
        let CloseDecision::Owner { tracker, .. } = &decision else {
            panic!("the fixture hands close ownership to the caller");
        };
        let mut held: Vec<_> = (0..extra).map(|_| tracker.token()).collect();
        let close_shared = shared.clone();
        let close =
            tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
        aborted.await.unwrap();
        assert!(!close.await.unwrap());
        held.truncate(extra - release_early);
        kept.push((held, physical_work));
    }
    assert!(
        shared.fatal.take().is_none(),
        "40 + 30 entries at deferral, {} still held",
        shared.deferred_work()
    );
    assert_eq!(shared.deferred_work(), 2 + 31);
    drop(kept);
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("both deferred closes finish once their work stops");
}

/// After its work stops, a deferred close emits under one fresh close budget and retires the
/// generation when that budget runs out.
#[tokio::test(start_paused = true)]
async fn a_deferred_close_emits_under_one_fresh_close_budget() {
    let CloseFixture {
        shared,
        generation,
        queue,
        route,
        decision,
        aborted,
        physical_work,
        ..
    } = fixture();
    let close_shared = shared.clone();
    let close =
        tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
    aborted.await.unwrap();
    assert!(!close.await.unwrap());
    let _reserve = shared.terminal_budget.try_charge(1 << 20).unwrap();
    let stopped = Instant::now();
    drop(physical_work);
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("deferred closes finish once their work stops");
    assert_eq!(Instant::now() - stopped, Duration::from_millis(100));
    assert!(generation.token.is_cancelled());
    assert_eq!(shared.registry.live_count(), 0);
    drop(queue);
}

#[tokio::test(start_paused = true)]
async fn post_abort_emission_shares_the_close_deadline() {
    let CloseFixture {
        shared,
        generation,
        queue,
        route,
        decision,
        aborted,
        physical_work,
        ..
    } = fixture();
    // The fallback terminal encodes from the terminal reserve; holding every byte of it, with
    // the writer's queue still open, makes the budget wait the only reason emission cannot
    // finish, so the shared post-abort deadline must be what ends it.
    let _terminal = shared.terminal_budget.try_charge(1 << 20).unwrap();
    let started = Instant::now();
    let close = tokio::spawn(async move { settle_route_work(&shared, route, decision).await });
    aborted.await.unwrap();
    // Spending half of the post-abort window before the work stops separates a shared deadline
    // (done at 200 ms) from a fresh emission budget (250 ms).
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(physical_work);
    assert!(close.await.unwrap());
    assert!(
        Instant::now() - started <= Duration::from_millis(200),
        "fallback emission exceeded the two shared close windows: {:?}",
        Instant::now() - started
    );
    assert!(
        generation.token.is_cancelled(),
        "undeliverable terminal must retire the generation"
    );
    drop(queue);
}

#[tokio::test(start_paused = true)]
async fn post_abort_snapshot_does_not_resettle_rejection() {
    for removed in [false, true] {
        let CloseFixture {
            shared,
            generation,
            mut queue,
            route,
            key,
            cancelled,
            settlement,
            decision,
            aborted,
            physical_work,
        } = fixture();
        let close_shared = shared.clone();
        let close =
            tokio::spawn(async move { settle_route_work(&close_shared, route, decision).await });
        aborted.await.unwrap();
        assert!(cancelled.is_cancelled());
        shared.registry.freeze_admission();
        let never_started = tokio::spawn(std::future::pending::<()>());
        assert!(
            shared
                .registry
                .register_dispatch(route, 1, never_started.abort_handle())
                .is_none()
        );
        never_started.abort();
        let _ = never_started.await;
        settle(
            &settlement,
            &shared.terminal_budget,
            &generation,
            route,
            key.2,
            Terminal::Error {
                code: "server_busy".to_owned(),
                message: "host is shutting down".to_owned(),
                retry_after_ms: None,
            },
        )
        .await;
        let rejected = queue.recv().await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&rejected.bytes[crate::wire::HEADER_LEN..])
                .unwrap()["code"],
            "server_busy"
        );
        if removed {
            generation.pending.lock().unwrap().remove(&key);
        }
        drop(physical_work);
        assert!(close.await.unwrap());
        assert!(
            queue.try_recv().is_err(),
            "rejection received a second terminal from the close snapshot (removed={removed})"
        );
    }
}

/// A registration-race rejection settles a credited request, so its credit must ride the
/// rejection frame to its block instead of returning when the settlement drops.
#[tokio::test]
async fn registration_race_rejection_carries_the_terminal_credit() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        route,
        key,
        ..
    } = fixture();
    let credit = generation
        .terminal_credits
        .clone()
        .try_acquire_owned()
        .unwrap();
    let before = generation.terminal_credits.available_permits();
    let settlement = Settlement::with_credit(Some(credit));
    settle(
        &settlement,
        &shared.terminal_budget,
        &generation,
        route,
        key.2,
        Terminal::Error {
            code: "unknown_channel".to_owned(),
            message: "no live route for this channel and epoch".to_owned(),
            retry_after_ms: None,
        },
    )
    .await;
    drop(settlement);
    let rejected = queue.recv().await.unwrap();
    assert_eq!(
        generation.terminal_credits.available_permits(),
        before,
        "the credit must still be held while the rejection frame is unreturned"
    );
    assert!(
        rejected.credit.is_some(),
        "the rejection frame carries the credit to its block"
    );
    drop(rejected);
    assert_eq!(generation.terminal_credits.available_permits(), before + 1);
}

/// A registration-race rejection runs off the connection reader: with the writer queue full,
/// `dispatch_request` returns at once, and the credited rejection still reaches the queue later.
#[tokio::test]
async fn registration_race_rejection_does_not_block_the_reader_on_a_full_writer_queue() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        ..
    } = fixture();
    let route = shared
        .registry
        .reserve(&generation, RouteClass::General)
        .unwrap();
    shared.registry.install_bound(route);
    shared.registry.freeze_admission();
    for corr in 100..108 {
        crate::dispatch::emit_error_terminal(
            &shared.terminal_budget,
            &generation,
            FrameId::control(corr),
            "filler",
            "fills the writer queue",
        )
        .await;
    }
    let before = generation.terminal_credits.available_permits();
    let (rings, lease) = lease_whose_return_succeeds();
    let budget = crate::wire::ByteBudget::new(16);
    let header = crate::wire::EnvelopeHeader {
        len: 1,
        ty: crate::wire::FrameType::Request,
        flags: crate::wire::response_flags(false, true),
        channel: route.channel,
        epoch: route.epoch,
        corr: 11,
    };
    let frame =
        crate::frame_channel::InboundFrame::new(header, lease, budget.try_charge(1).unwrap());
    tokio::time::timeout(
        Duration::from_millis(200),
        crate::dispatch::dispatch_request(&shared, &generation, frame),
    )
    .await
    .expect("the reader must not wait on writer-queue capacity for a rejection");
    for _ in 0..8 {
        drop(queue.recv().await.unwrap());
    }
    let rejected = queue.recv().await.unwrap();
    assert!(
        rejected.credit.is_some(),
        "the rejection frame carries the credit to its block"
    );
    assert_eq!(generation.terminal_credits.available_permits(), before - 1);
    drop(rejected);
    assert_eq!(generation.terminal_credits.available_permits(), before);
    assert!(!generation.token.is_cancelled());
    drop(rings);
}

/// A request that loses the registration race still returns its lease before any rejection; a
/// failed return doorbell retires the generation instead of queueing a rejection over a
/// transport that cannot be woken.
#[tokio::test]
async fn a_request_losing_the_registration_race_retires_on_a_failed_return() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        ..
    } = fixture();
    let route = shared
        .registry
        .reserve(&generation, RouteClass::General)
        .unwrap();
    shared.registry.install_bound(route);
    shared.registry.freeze_admission();
    let mut leases = leases_whose_return_wake_fails();
    let budget = crate::wire::ByteBudget::new(16);
    let header = crate::wire::EnvelopeHeader {
        len: 1,
        ty: crate::wire::FrameType::Request,
        flags: crate::wire::response_flags(false, true),
        channel: route.channel,
        epoch: route.epoch,
        corr: 9,
    };
    let frame = crate::frame_channel::InboundFrame::new(
        header,
        leases.pop().unwrap(),
        budget.try_charge(1).unwrap(),
    );
    crate::dispatch::dispatch_request(&shared, &generation, frame).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("deferred closes finish once their work stops");
    assert!(
        generation.token.is_cancelled(),
        "the failed return is a transport fault, not a rejectable request"
    );
    assert!(
        queue.try_recv().is_err(),
        "no rejection is queued over a transport that cannot be woken"
    );
    drop(leases);
}

/// One committed one-byte frame received through a live ring pair; the pair is returned so the
/// producer's doorbell stays open and the lease's return wake succeeds.
fn lease_whose_return_succeeds() -> (
    shm_transport::backend::ring::DuplexRing,
    shm_transport::lease::PayloadLease,
) {
    use shm_transport::backend::ring::{DuplexRing, body_header};
    let rings = DuplexRing::create(&crate::ring_transport::ring_profile()).unwrap();
    let consumer = rings.first.attachment().unwrap().attach().unwrap();
    let mut reservation = rings.first.try_reserve(1, body_header(1).unwrap()).unwrap();
    reservation.write(&[1]).unwrap();
    reservation.commit(1).unwrap();
    let lease = consumer.try_receive().unwrap().unwrap();
    (rings, lease)
}

/// Holds every block of the smallest ordinary class so the producer can arm a capacity wait;
/// dropping the producer afterwards closes its doorbell end, so the first return's wake fails.
/// Every lease stays held until the caller drops it: a drop before the frame under test would
/// consume the parked marker and the later return would send nothing.
fn leases_whose_return_wake_fails() -> Vec<shm_transport::lease::PayloadLease> {
    use shm_transport::backend::ring::{DuplexRing, body_header};
    use shm_transport::pool::Inventory;
    let rings = DuplexRing::create(&crate::ring_transport::ring_profile()).unwrap();
    let consumer = rings.first.attachment().unwrap().attach().unwrap();
    let class_count = rings
        .first
        .geometry()
        .class(shm_transport::pool::BlockClass::Ordinary(0))
        .count as usize;
    let held: Vec<_> = (0..class_count)
        .map(|_| {
            let mut reservation = rings.first.try_reserve(1, body_header(1).unwrap()).unwrap();
            reservation.write(&[1]).unwrap();
            reservation.commit(1).unwrap();
            consumer.try_receive().unwrap().unwrap()
        })
        .collect();
    assert_eq!(
        rings.first.arm_capacity_wait(Inventory::Ordinary, 1),
        Ok(true)
    );
    drop(rings);
    held
}

/// A pure-header frame whose lease cannot ring the return doorbell is a transport fault: the
/// read loop must exit at that frame instead of applying it and reading on.
#[tokio::test]
async fn a_pure_header_frame_with_a_failed_return_wake_ends_the_read_loop() {
    let CloseFixture {
        shared,
        generation,
        route,
        key,
        settlement,
        ..
    } = fixture();
    // The fixture's route is already closing; watch a fresh pending entry instead.
    let cancelled = CancellationToken::new();
    generation.pending.lock().unwrap().insert(
        key,
        PendingEntry {
            cancel: cancelled.clone(),
            settlement,
            _pending_permit: shared.pending_permits.clone().try_acquire_owned().unwrap(),
        },
    );
    let mut leases = leases_whose_return_wake_fails();
    let budget = crate::wire::ByteBudget::new(16);
    let pong = crate::wire::EnvelopeHeader {
        len: 0,
        ty: crate::wire::FrameType::Pong,
        flags: crate::wire::pure_header_flags(),
        channel: 0,
        epoch: 0,
        corr: 9,
    };
    let cancel = crate::wire::EnvelopeHeader {
        len: 0,
        ty: crate::wire::FrameType::Cancel,
        flags: crate::wire::pure_header_flags(),
        channel: route.channel,
        epoch: route.epoch,
        corr: key.2,
    };
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    for header in [pong, cancel] {
        let frame = crate::frame_channel::InboundFrame::new(
            header,
            leases.pop().unwrap(),
            budget.try_charge(0).unwrap(),
        );
        tx.try_send(Ok(crate::frame_channel::InboundEvent::Frame(frame)))
            .unwrap();
    }
    drop(tx);
    let exit = crate::connection::read_loop(
        &shared,
        &generation,
        crate::ring_transport::ShmReceiver::from_channel(rx),
    )
    .await;
    assert!(
        matches!(exit, crate::connection::ReadExit::Peer),
        "a failed return doorbell on the Pong ends the generation: {exit:?}"
    );
    assert!(
        !cancelled.is_cancelled(),
        "the Cancel behind the faulted Pong must not be applied"
    );
    drop(leases);
}

/// A request refused before its copy still returns its lease; a failed return doorbell must
/// retire the generation instead of queueing a rejection over a broken transport.
#[tokio::test]
async fn a_rejected_request_whose_return_wake_fails_retires_the_generation() {
    let CloseFixture {
        shared,
        generation,
        route,
        mut queue,
        ..
    } = fixture();
    shared.shutdown.cancel();
    let mut leases = leases_whose_return_wake_fails();
    let budget = crate::wire::ByteBudget::new(16);
    let header = crate::wire::EnvelopeHeader {
        len: 1,
        ty: crate::wire::FrameType::Request,
        flags: crate::wire::response_flags(false, true),
        channel: route.channel,
        epoch: route.epoch,
        corr: 1,
    };
    let frame = crate::frame_channel::InboundFrame::new(
        header,
        leases.pop().unwrap(),
        budget.try_charge(1).unwrap(),
    );
    crate::dispatch::dispatch_request(&shared, &generation, frame).await;
    assert!(
        generation.token.is_cancelled(),
        "the failed return is a transport fault, not a rejectable request"
    );
    assert!(
        queue.try_recv().is_err(),
        "no rejection is queued over a transport that cannot be woken"
    );
    drop(leases);
}

/// A request whose route closes after registration is cancelled before its private copy runs;
/// the lease returns explicitly, and a failed return doorbell retires the generation instead of
/// queueing a cancellation terminal over a transport that cannot be woken.
#[tokio::test]
async fn a_request_cancelled_by_route_close_before_its_copy_retires_on_a_failed_return() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        ..
    } = fixture();
    let route = shared
        .registry
        .reserve(&generation, RouteClass::General)
        .unwrap();
    shared.registry.install_bound(route);
    let mut leases = leases_whose_return_wake_fails();
    let budget = crate::wire::ByteBudget::new(16);
    let header = crate::wire::EnvelopeHeader {
        len: 1,
        ty: crate::wire::FrameType::Request,
        flags: crate::wire::response_flags(false, true),
        channel: route.channel,
        epoch: route.epoch,
        corr: 7,
    };
    let frame = crate::frame_channel::InboundFrame::new(
        header,
        leases.pop().unwrap(),
        budget.try_charge(1).unwrap(),
    );
    // Registration completes here; the copy runs on the spawned task, which the current-thread
    // test runtime cannot poll before this test yields, so the route closes first.
    crate::dispatch::dispatch_request(&shared, &generation, frame).await;
    let _decision = shared.registry.begin_close(route);
    tokio::time::timeout(Duration::from_secs(1), async {
        shared.tracker.close();
        shared.tracker.wait().await;
    })
    .await
    .expect("deferred closes finish once their work stops");
    assert!(
        generation.token.is_cancelled(),
        "the failed return is a transport fault, not a cancellable request"
    );
    assert!(
        queue.try_recv().is_err(),
        "no terminal is queued over a transport that cannot be woken"
    );
    drop(leases);
}
