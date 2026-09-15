use super::*;
use crate::connection::{PendingEntry, PendingKey};
use crate::dispatch::{Settlement, emit_pending_rejection, settle_route_work};
use crate::frame_channel::{SenderQueue, frame_sender};
use crate::handler::{
    BindOutcome, RequestCtx, RequestOutcome, RouteClass, RouteHandle, RouteIdentity, RouteTarget,
};
use crate::routing::CloseDecision;
use crate::wire::FrameId;

struct UnusedHandler;
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
    async fn route_gone(&self, _: RouteHandle) {}
    async fn health(&self) -> HealthReport {
        HealthReport::ok()
    }
    async fn shutdown(&self) {}
}

fn shared() -> Arc<HostShared<UnusedHandler>> {
    Arc::new(HostShared {
        handler: Arc::new(UnusedHandler),
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
    let shared = shared();
    let token = CancellationToken::new();
    let (writer, queue) = frame_sender(8, token.clone(), Duration::from_secs(30));
    let generation = Arc::new(GenerationCore {
        id: 1,
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
    let (tracker, _, cancelled) = shared.registry.route_tracker(route, 1).unwrap();
    let key = (route.channel, route.epoch, 1);
    let settlement = Settlement::with_credit(None);
    generation.pending.lock().unwrap().insert(
        key,
        PendingEntry {
            cancel: cancelled.clone(),
            settlement: settlement.clone(),
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
        .register_dispatch(route, 1, outer.abort_handle())
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
async fn live_work_after_both_close_windows_refuses_cleanup() {
    let CloseFixture {
        shared,
        generation,
        mut queue,
        route,
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
        "held physical work must refuse cleanup"
    );
    assert_eq!(Instant::now() - started, Duration::from_millis(200));
    assert!(shared.shutdown.is_cancelled());
    assert!(queue.try_recv().is_err());
    assert!(!generation.pending.lock().unwrap().is_empty());
    drop(physical_work);
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
        emit_pending_rejection(
            &shared,
            &generation,
            &settlement,
            FrameId::routed(route, key.2),
            "server_busy",
            "host is shutting down",
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
    emit_pending_rejection(
        &shared,
        &generation,
        &settlement,
        FrameId::routed(route, key.2),
        "unknown_channel",
        "no live route for this channel and epoch",
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

/// Holds every block of the smallest ordinary class so the producer can arm a capacity wait;
/// dropping the producer afterwards closes its doorbell end, so the first return's wake fails.
/// Every lease stays held until the caller drops it: a drop before the frame under test would
/// consume the parked marker and the later return would send nothing.
fn leases_whose_return_wake_fails() -> Vec<shm_transport::lease::PayloadLease> {
    use shm_transport::backend::ring::{DuplexRing, wire_v3_header};
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
            let mut reservation = rings
                .first
                .try_reserve(1, wire_v3_header(1).unwrap())
                .unwrap();
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
        },
    );
    let mut leases = leases_whose_return_wake_fails();
    let budget = crate::wire::ByteBudget::new(16);
    let pong = crate::wire::EnvelopeHeader {
        len: 0,
        ver: crate::wire::PROTOCOL_VERSION,
        ty: crate::wire::FrameType::Pong,
        flags: crate::wire::pure_header_flags(),
        channel: 0,
        epoch: 0,
        corr: 9,
    };
    let cancel = crate::wire::EnvelopeHeader {
        len: 0,
        ver: crate::wire::PROTOCOL_VERSION,
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
        ver: crate::wire::PROTOCOL_VERSION,
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
