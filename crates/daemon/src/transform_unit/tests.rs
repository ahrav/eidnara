#[path = "host_tests.rs"]
mod host_tests;

use super::*;
use crate::transform_unit::{DetachedRunner, UnitOutcome, UnitRunner};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;
use tokio::sync::oneshot;

#[test]
fn spent_meter_cannot_reenter_admitted_body() {
    let body = br#"{"kind":"transform"}"#;
    let pool = TestPool::with_capacity(footprint_of(body));
    let meter = ResidentMeter::new(&pool);
    admit_body(body, &meter).unwrap();
    let decoded: Value = decode_metered(body, &meter).unwrap();
    let charges = meter.take_charges();
    let calls = pool.reserve_calls();
    assert!(admit_body(body, &meter).is_err());
    assert_eq!(pool.reserve_calls(), calls);
    drop(decoded);
    drop(charges);
    assert_pool_released(&pool);
}

async fn watchdog<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("test operation must finish before the watchdog")
}

struct BlockingGate {
    entered: oneshot::Receiver<()>,
    // Dropping the sender also releases blocking_recv during assertion unwinding.
    _release: oneshot::Sender<()>,
}

impl BlockingGate {
    fn new() -> (Self, Box<dyn FnOnce() + Send>) {
        let (entered_tx, entered) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let hook = Box::new(move || {
            let _ = entered_tx.send(());
            let _ = released.blocking_recv();
        });
        (
            Self {
                entered,
                _release: release,
            },
            hook,
        )
    }

    async fn wait_entered(&mut self) {
        watchdog(&mut self.entered)
            .await
            .expect("blocking worker must reach the gate");
    }
}

/// The tracker owns each join independently of the handler's response waiter.
#[derive(Default)]
pub(super) struct JoinedUnitRunner {
    worker: DetachedRunner,
    joins: TaskTracker,
    before_unit: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
    submitted: AtomicUsize,
    pub(super) completed: Arc<AtomicUsize>,
}

impl UnitRunner for JoinedUnitRunner {
    fn run_unit(
        &self,
        work: Box<dyn FnOnce() -> UnitOutcome + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<UnitOutcome, BlockingWorkFailed>> + Send + 'static>>
    {
        let gate = self.before_unit.lock().unwrap().pop_front();
        self.submitted.fetch_add(1, Ordering::SeqCst);
        let joined = self.worker.run_unit(Box::new(move || {
            if let Some(gate) = gate {
                gate();
            }
            work()
        }));
        let (send, receive) = oneshot::channel();
        let completed = Arc::clone(&self.completed);
        self.joins.spawn(async move {
            let result = joined.await;
            completed.fetch_add(1, Ordering::SeqCst);
            let _ = send.send(result);
        });
        Box::pin(async move { receive.await.expect("independent join owner must finish") })
    }

    fn run_step(
        &self,
        work: Box<dyn FnOnce() + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlockingWorkFailed>> + Send + 'static>> {
        self.worker.run_step(work)
    }

    fn cancel_signal(&self) -> CancelSignal {
        self.worker.cancel_signal()
    }
}

impl JoinedUnitRunner {
    async fn join_all(&self) {
        self.joins.close();
        watchdog(self.joins.wait()).await;
    }
}

// Unlike handle_transform_with_runner, the caller retains this pool to inspect its charge
// after the handler waiter is gone. Both use the metered body entry, not Value dispatch.
async fn metered_transform(
    handler: &Handler,
    request: Value,
    runner: &dyn UnitRunner,
    pool: &TestPool,
) -> PreparedOutcome {
    let body = serde_json::to_vec(&request).unwrap();
    let meter = ResidentMeter::new(pool);
    if let Err(outcome) = admit_body(&body, &meter) {
        return outcome;
    }
    let probe = lane_probe(&body);
    let entry = PassEntry {
        core: &handler.core,
        route: test_route(7),
        probe: probe.as_ref(),
        meter: &meter,
        runner,
    };
    handler.dispatch_body(&entry, &body).await.1
}

fn assert_pool_released(pool: &TestPool) {
    let _all = pool
        .try_reserve(pool.capacity())
        .expect("every resident byte must return after the worker is joined");
    assert!(
        pool.try_reserve(1).is_none(),
        "release must not grow the pool"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn aborted_waiter_preserves_commit_bookkeeping_and_worker_charges() {
    let (handler, store, dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let handler = Arc::new(handler);
    let runner = Arc::new(JoinedUnitRunner::default());
    let first = request(vec![ck("m1", 1, "keep this committed message")]);
    // Exact capacity makes even one prematurely released byte observable.
    let pool = Arc::new(TestPool::with_capacity(footprint_of(
        &serde_json::to_vec(&first).unwrap(),
    )));
    let stale_date = "Today's date: Fri Jan 01 2016";
    handler
        .guidance_dates
        .lock()
        .unwrap()
        .insert("ses".to_string(), stale_date.to_string());
    assert!(store.load("ses").unwrap().row_version.is_none());
    let (mut gate, hook) = BlockingGate::new();
    *handler.after_transform_commit.lock().unwrap() = Some(hook);
    let waiter = {
        let handler = Arc::clone(&handler);
        let runner = Arc::clone(&runner);
        let pool = Arc::clone(&pool);
        let first = first.clone();
        tokio::spawn(async move { metered_transform(&handler, first, &*runner, &pool).await })
    };
    gate.wait_entered().await;

    let committed = store.load("ses").unwrap();
    assert!(committed.row_version.is_some());
    assert!(committed.meta.initialized);
    assert_eq!(committed.meta.guidance_date, stale_date);
    assert!(
        !handler
            .transform_session_roots
            .lock()
            .unwrap()
            .contains_key("ses")
    );
    assert_eq!(handler.guidance_dates.lock().unwrap()["ses"], stale_date);
    assert!(pool.try_reserve(1).is_none());
    assert_eq!(handler.transform_units.available_permits(), 3);

    waiter.abort();
    assert!(watchdog(waiter).await.unwrap_err().is_cancelled());
    runner.worker.cancel.cancel();
    assert_eq!(
        runner.joins.len(),
        1,
        "waiter abort is not worker completion"
    );
    assert!(
        pool.try_reserve(1).is_none(),
        "the parked worker still owns its request"
    );
    assert_eq!(handler.transform_units.available_permits(), 3);
    assert_eq!(
        store.load("ses").unwrap().row_version,
        committed.row_version
    );

    let released_at_ms = now_ms().max(0) as u64;
    drop(gate);
    runner.join_all().await;
    assert!(
        DISPATCH_HEALTH
            .last_dispatch_completed_at_ms
            .load(Ordering::Relaxed)
            >= released_at_ms,
        "the unit's end must advance the lane heartbeat after its waiter was aborted"
    );
    assert!(
        handler.transform_session_roots.lock().unwrap()["ses"].contains(&canonical_root(&project))
    );
    assert!(!handler.guidance_dates.lock().unwrap().contains_key("ses"));
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert_pool_released(&pool);

    *handler.store.lock().unwrap() = None;
    drop(store);
    let store = Arc::new(
        MemoryStore::open(&dev_descriptor_at(
            dir.path().join("data").to_str().unwrap(),
        ))
        .unwrap(),
    );
    let persisted = store.load("ses").unwrap();
    assert!(persisted.row_version >= committed.row_version);
    assert_eq!(persisted.core, committed.core);
    assert_eq!(persisted.meta.guidance_date, stale_date);
    handler.install_store_for_test(Arc::clone(&store));

    let runner = JoinedUnitRunner::default();
    let mut next = first;
    next["render_config"] = json!("cfg1");
    let response = tool_body(watchdog(metered_transform(&handler, next, &runner, &pool)).await);
    runner.join_all().await;
    assert_eq!(response["action"], "HARD");
    assert_eq!(response["committed"], true);
    let fresh = store.load("ses").unwrap().meta.guidance_date;
    assert_ne!(fresh, stale_date);
    let trace = store.load_pass_trace("ses").unwrap().unwrap();
    assert_eq!(trace.receive_count, 2);
    // The receive breadcrumb and guidance computation use the same pass timestamp.
    assert_eq!(
        fresh,
        handler.guidance_date_line_for_ms(trace.last_received_at_ms),
        "the next pass must compute a fresh date rather than reuse the committed pin"
    );
    assert_pool_released(&pool);
}

#[tokio::test(flavor = "current_thread")]
async fn aborted_page_waiter_keeps_applying_and_staged_bytes_until_worker_finishes() {
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let handler = Arc::new(handler);
    let runner = Arc::new(JoinedUnitRunner::default());
    let unpaged = request(vec![ck("m1", 1, "assembled page content")]);
    let mut page = unpaged.clone();
    page["transform_page_id"] = json!("held-page");
    page["transform_generation"] = json!(0);
    page["transform_page_index"] = json!(0);
    page["transform_page_total"] = json!(1);
    page["transform_page_complete"] = json!(true);
    page["transform_page_digest"] = json!(transform_page_content_digest(&page));
    let body = serde_json::to_vec(&page).unwrap();
    let staged_bytes = body.len();
    let pool = Arc::new(TestPool::with_capacity(footprint_of(&body)));
    let (mut gate, hook) = BlockingGate::new();
    *handler.after_transform_commit.lock().unwrap() = Some(hook);
    let waiter = {
        let handler = Arc::clone(&handler);
        let runner = Arc::clone(&runner);
        let pool = Arc::clone(&pool);
        let page = page.clone();
        tokio::spawn(async move { metered_transform(&handler, page, &*runner, &pool).await })
    };
    gate.wait_entered().await;
    let assert_applying = || {
        let pages = handler.transform_pages.lock().unwrap();
        assert!(matches!(
            &pages.sessions["ses"].phase,
            TransformPagePhase::Applying { transform_id, bytes }
                if transform_id == "held-page" && *bytes == staged_bytes
        ));
        assert_eq!(pages.total_staged_bytes, staged_bytes);
        assert_eq!(pages.pending_transform_count, 1);
        assert!(pages.sessions["ses"].completed.is_none());
    };
    assert_applying();
    assert!(store.load("ses").unwrap().row_version.is_some());
    assert!(pool.try_reserve(1).is_none());

    waiter.abort();
    assert!(watchdog(waiter).await.unwrap_err().is_cancelled());
    runner.worker.cancel.cancel();
    assert_applying();
    assert_eq!(runner.joins.len(), 1);
    assert!(pool.try_reserve(1).is_none());
    assert_eq!(handler.transform_units.available_permits(), 3);
    for blocked in [unpaged.clone(), page.clone()] {
        let outcome =
            watchdog(handler.handle_transform_with_runner(test_route(7), blocked, &*runner)).await;
        assert_eq!(error_code(outcome), "authority_transform_page_in_progress");
        assert_applying();
    }
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 1);

    drop(gate);
    runner.join_all().await;
    {
        let pages = handler.transform_pages.lock().unwrap();
        assert!(!pages.sessions.contains_key("ses"));
        assert_eq!(pages.total_staged_bytes, 0);
        assert_eq!(pages.pending_transform_count, 0);
        assert_eq!(pages.completed_bytes, 0);
    }
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert_pool_released(&pool);

    let runner = JoinedUnitRunner::default();
    let response = tool_body(watchdog(metered_transform(&handler, unpaged, &runner, &pool)).await);
    assert!(response["action"].is_string(), "{response}");
    page["transform_page_id"] = json!("next-page");
    let response = tool_body(watchdog(metered_transform(&handler, page, &runner, &pool)).await);
    assert!(response["action"].is_string(), "{response}");
    runner.join_all().await;
    let pages = handler.transform_pages.lock().unwrap();
    assert!(matches!(
        pages.sessions["ses"].phase,
        TransformPagePhase::Idle
    ));
    assert!(pages.completed("ses", "next-page").is_some());
    assert_eq!(pages.total_staged_bytes, 0);
    assert_eq!(pages.pending_transform_count, 0);
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert_pool_released(&pool);
}

#[test]
fn stale_apply_release_preserves_newer_attempt_and_matching_release_runs_once() {
    // Two live attempts retain 37 and 11 bytes. The second exposes an extra decrement
    // that zero-saturating counters would hide with only one attempt.
    let mut pages = TransformPageCoordinator {
        total_staged_bytes: 48,
        pending_transform_count: 2,
        ..TransformPageCoordinator::default()
    };
    pages.set_phase(
        "ses",
        TransformPagePhase::Applying {
            transform_id: "new-attempt".to_string(),
            bytes: 37,
        },
    );
    pages.set_phase(
        "other",
        TransformPagePhase::Applying {
            transform_id: "other-attempt".to_string(),
            bytes: 11,
        },
    );

    assert!(!pages.release_applying("ses", "old-attempt"));
    assert!(matches!(
        &pages.sessions["ses"].phase,
        TransformPagePhase::Applying { transform_id, bytes }
            if transform_id == "new-attempt" && *bytes == 37
    ));
    assert_eq!(pages.total_staged_bytes, 48);
    assert_eq!(pages.pending_transform_count, 2);

    assert!(pages.release_applying("ses", "new-attempt"));
    assert!(!pages.sessions.contains_key("ses"));
    assert_eq!(pages.total_staged_bytes, 11);
    assert_eq!(pages.pending_transform_count, 1);
    assert!(!pages.release_applying("ses", "new-attempt"));
    assert_eq!(pages.total_staged_bytes, 11);
    assert_eq!(pages.pending_transform_count, 1);
    assert!(pages.release_applying("other", "other-attempt"));
    assert_eq!(pages.total_staged_bytes, 0);
    assert_eq!(pages.pending_transform_count, 0);
    assert!(pages.sessions.is_empty());
}

#[test]
fn discard_leaves_an_applying_phase_to_its_guard() {
    let mut pages = TransformPageCoordinator::default();
    let staged = pages
        .stage(
            "ses",
            "t-1".to_string(),
            1,
            0,
            1,
            "d0".to_string(),
            json!({"messages": []}),
            37,
            true,
            1,
            Instant::now(),
        )
        .unwrap();
    assert!(matches!(staged, TransformPageStageAction::Apply { .. }));
    assert_eq!(pages.total_staged_bytes, 37);
    assert_eq!(pages.pending_transform_count, 1);

    assert_eq!(pages.discard("ses"), None);
    assert!(matches!(
        &pages.sessions["ses"].phase,
        TransformPagePhase::Applying { transform_id, bytes }
            if transform_id == "t-1" && *bytes == 37
    ));
    assert_eq!(pages.total_staged_bytes, 37);
    assert_eq!(pages.pending_transform_count, 1);
    assert!(matches!(
        pages.stage(
            "ses",
            "t-2".to_string(),
            1,
            0,
            1,
            "d1".to_string(),
            json!({"messages": []}),
            5,
            true,
            2,
            Instant::now(),
        ),
        Err(TransformPageStageError::InProgress)
    ));

    assert!(pages.release_applying("ses", "t-1"));
    assert_eq!(pages.total_staged_bytes, 0);
    assert_eq!(pages.pending_transform_count, 0);
    assert!(pages.sessions.is_empty());
}

#[test]
fn discard_during_apply_drops_the_retained_response_and_retains_none_afterwards() {
    fn stage_final(pages: &mut TransformPageCoordinator, id: &str) {
        let staged = pages
            .stage(
                "ses",
                id.to_string(),
                1,
                0,
                1,
                "d".to_string(),
                json!({"messages": []}),
                7,
                true,
                1,
                Instant::now(),
            )
            .unwrap();
        assert!(matches!(staged, TransformPageStageAction::Apply { .. }));
    }
    fn finish(pages: &mut TransformPageCoordinator, id: &str) {
        pages.finish_apply(
            "ses",
            id.to_string(),
            1,
            1,
            "d".to_string(),
            String::new(),
            Some(PreparedOutput::cached_bytes(vec![0; 5])),
        );
    }

    let mut pages = TransformPageCoordinator::default();
    stage_final(&mut pages, "a");
    finish(&mut pages, "a");
    assert!(pages.completed("ses", "a").is_some());
    stage_final(&mut pages, "b");
    assert_eq!(pages.discard("ses"), None);
    assert_eq!(pages.completed_bytes, 0);
    assert_eq!(pages.total_staged_bytes, 7);
    assert!(pages.release_applying("ses", "b"));
    assert!(pages.completed("ses", "a").is_none());
    assert!(pages.sessions.is_empty());

    let mut pages = TransformPageCoordinator::default();
    stage_final(&mut pages, "a");
    finish(&mut pages, "a");
    stage_final(&mut pages, "b");
    assert_eq!(pages.discard("ses"), None);
    finish(&mut pages, "b");
    assert!(pages.completed("ses", "b").is_none());
    assert_eq!(pages.completed_bytes, 0);
    assert_eq!(pages.total_staged_bytes, 0);
    assert!(pages.sessions.is_empty());

    let mut pages = TransformPageCoordinator::default();
    stage_final(&mut pages, "c");
    finish(&mut pages, "c");
    assert!(pages.completed("ses", "c").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn four_parked_units_keep_fifth_waiter_off_the_blocking_pool() {
    assert_eq!(TRANSFORM_UNITS_AT_ONCE, 4);
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let handler = Arc::new(handler);
    let runner = Arc::new(JoinedUnitRunner::default());
    let mut parked = Vec::new();
    for index in 0..4 {
        let route = test_route(20 + index);
        let session = format!("unit-{index}");
        handler.bind_route(route, binding(project.to_str().unwrap(), &session));
        let mut input = request(vec![ck("m1", 1, "one bounded unit")]);
        input["session_id"] = json!(session);
        let (mut gate, hook) = BlockingGate::new();
        runner.before_unit.lock().unwrap().push_back(hook);
        let waiter = {
            let handler = Arc::clone(&handler);
            let runner = Arc::clone(&runner);
            tokio::spawn(async move {
                handler
                    .handle_transform_with_runner(route, input, &*runner)
                    .await
            })
        };
        gate.wait_entered().await;
        parked.push((gate, waiter));
    }
    assert_eq!(handler.transform_units.available_permits(), 0);
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
    assert_eq!(runner.joins.len(), 4);

    let input = request(vec![ck("m1", 1, "fifth unit")]);
    let pool = TestPool::with_capacity(footprint_of(&serde_json::to_vec(&input).unwrap()));
    let mut fifth = Box::pin(metered_transform(&handler, input.clone(), &*runner, &pool));
    // One explicit poll reaches the exhausted permit acquisition without a scheduling race.
    poll_fn(|cx| {
        assert!(fifth.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
    assert!(
        pool.try_reserve(1).is_none(),
        "the queued request retains its decode charge"
    );
    drop(fifth);
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
    assert_pool_released(&pool);
    assert!(store.load("ses").unwrap().row_version.is_none());
    assert!(matches!(
        handler.transform_snapshots.lock().unwrap().get("ses"),
        TransformSnapshotLookup::Missing
    ));
    assert_eq!(handler.transform_units.available_permits(), 0);

    let mut replacement = Box::pin(metered_transform(&handler, input, &*runner, &pool));
    poll_fn(|cx| {
        assert!(replacement.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(pool.try_reserve(1).is_none());
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
    assert_eq!(handler.transform_units.available_permits(), 0);

    // The other three gates stay closed so only permit admission can unblock the replacement.
    let (gate, first_waiter) = parked.remove(0);
    drop(gate);
    let (first, replacement) = watchdog(async { tokio::join!(first_waiter, replacement) }).await;
    assert_eq!(tool_body(first.unwrap())["committed"], true);
    assert_eq!(tool_body(replacement)["committed"], true);
    assert!(store.load("ses").unwrap().row_version.is_some());
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 5);
    assert_eq!(runner.completed.load(Ordering::SeqCst), 2);
    assert_eq!(runner.joins.len(), 3);
    assert_eq!(handler.transform_units.available_permits(), 1);
    assert_pool_released(&pool);
    for index in 1..4 {
        assert!(
            store
                .load(&format!("unit-{index}"))
                .unwrap()
                .row_version
                .is_none()
        );
    }

    for (gate, waiter) in parked {
        drop(gate);
        let response = tool_body(watchdog(waiter).await.unwrap());
        assert_eq!(response["committed"], true);
    }
    runner.join_all().await;
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 5);
    assert_eq!(runner.completed.load(Ordering::SeqCst), 5);
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert_pool_released(&pool);
}

#[tokio::test(flavor = "current_thread")]
async fn admission_refuses_the_request_past_the_waiter_bound_and_declares_it() {
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let handler = Arc::new(handler);
    let runner = Arc::new(JoinedUnitRunner::default());
    let mut parked = Vec::new();
    for index in 0..TRANSFORM_UNITS_AT_ONCE {
        let route = test_route(20 + index as u16);
        let session = format!("unit-{index}");
        handler.bind_route(route, binding(project.to_str().unwrap(), &session));
        let mut input = request(vec![ck("m1", 1, "one bounded unit")]);
        input["session_id"] = json!(session);
        let (mut gate, hook) = BlockingGate::new();
        runner.before_unit.lock().unwrap().push_back(hook);
        let waiter = {
            let handler = Arc::clone(&handler);
            let runner = Arc::clone(&runner);
            tokio::spawn(async move {
                handler
                    .handle_transform_with_runner(route, input, &*runner)
                    .await
            })
        };
        gate.wait_entered().await;
        parked.push((gate, waiter));
    }
    assert_eq!(handler.transform_units.available_permits(), 0);

    let input = request(vec![ck("m1", 1, "queued unit")]);
    let pool = TestPool::unbounded();
    let mut waiting = Vec::new();
    for _ in TRANSFORM_UNITS_AT_ONCE..TRANSFORM_ADMISSION_PERMITS {
        let mut queued = Box::pin(metered_transform(&handler, input.clone(), &*runner, &pool));
        poll_fn(|cx| {
            assert!(queued.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        waiting.push(queued);
    }
    assert_eq!(
        runner.submitted.load(Ordering::SeqCst),
        TRANSFORM_UNITS_AT_ONCE
    );

    let refused_pool = TestPool::with_capacity(footprint_of(&serde_json::to_vec(&input).unwrap()));
    let refused = watchdog(metered_transform(
        &handler,
        input.clone(),
        &*runner,
        &refused_pool,
    ))
    .await;
    assert_eq!(error_code(refused), "queue_full");
    assert_pool_released(&refused_pool);
    let refused_route = test_route(40);
    handler.bind_route(refused_route, binding(project.to_str().unwrap(), "refused"));
    let mut refused_input = request(vec![ck("m1", 1, "refused unit")]);
    refused_input["session_id"] = json!("refused");
    refused_input["prompt_surface_config_identity"] = json!("refused-identity");
    let refused =
        watchdog(handler.handle_transform_with_runner(refused_route, refused_input, &*runner))
            .await;
    assert_eq!(error_code(refused), "queue_full");
    assert!(
        !handler
            .transform_route_channels
            .lock()
            .unwrap()
            .contains_key(&refused_route)
    );
    assert!(
        !handler
            .prompt_surface_epochs
            .lock()
            .unwrap()
            .contains_key("refused")
    );
    assert_eq!(
        runner.submitted.load(Ordering::SeqCst),
        TRANSFORM_UNITS_AT_ONCE
    );
    assert!(matches!(
        handler.transform_snapshots.lock().unwrap().get("ses"),
        TransformSnapshotLookup::Missing
    ));

    drop(waiting.pop());
    let mut readmitted = Box::pin(metered_transform(&handler, input, &*runner, &pool));
    poll_fn(|cx| {
        assert!(readmitted.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    waiting.push(readmitted);

    drop(waiting);
    for (gate, waiter) in parked {
        drop(gate);
        assert_eq!(
            tool_body(watchdog(waiter).await.unwrap())["committed"],
            true
        );
    }
    runner.join_all().await;
    assert_eq!(
        handler.transform_units.available_permits(),
        TRANSFORM_UNITS_AT_ONCE
    );
    assert_eq!(
        handler.transform_admission.available_permits(),
        TRANSFORM_ADMISSION_PERMITS
    );
    assert!(store.load("ses").unwrap().row_version.is_none());
    assert_eq!(
        Handler::new().resources().general_task_hold_bound,
        TRANSFORM_ADMISSION_PERMITS
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_unit_releases_its_charges_without_store_work() {
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let runner = JoinedUnitRunner::default();
    runner.worker.cancel.cancel();
    let pool = TestPool::with_capacity(64 * 1024);
    let outcome = watchdog(metered_transform(
        &handler,
        request(vec![ck("m1", 1, "cancelled before the unit")]),
        &runner,
        &pool,
    ))
    .await;
    runner.join_all().await;
    assert_eq!(error_code(outcome), "cancelled");
    assert!(store.load("ses").unwrap().row_version.is_none());
    assert!(store.load_pass_trace("ses").unwrap().is_none());
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert_pool_released(&pool);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_unit_keeps_the_prior_ready_snapshot() {
    let (handler, _store, _dir, _project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let runner = JoinedUnitRunner::default();
    let pool = TestPool::unbounded();
    let input = request(vec![ck("m1", 1, "first full pass")]);
    let first = watchdog(metered_transform(&handler, input.clone(), &runner, &pool)).await;
    assert_eq!(tool_body(first)["committed"], true);
    assert!(matches!(
        handler.transform_snapshots.lock().unwrap().get("ses"),
        TransformSnapshotLookup::Ready(_)
    ));

    runner.worker.cancel.cancel();
    let cancelled = watchdog(metered_transform(&handler, input, &runner, &pool)).await;
    runner.join_all().await;
    assert_eq!(error_code(cancelled), "cancelled");
    assert!(matches!(
        handler.transform_snapshots.lock().unwrap().get("ses"),
        TransformSnapshotLookup::Ready(_)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn emergency_cancellation_between_units_preserves_commit_and_releases_scratch() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = big_messages();
    let first = watchdog(call_transform(&handler, messages.clone())).await;
    assert_eq!(first["history_summarizer"]["fired"], true);
    wait_for_count(&producer.starts, 1).await;
    wait_for_count(&producer.await_outputs, 1).await;
    let seed_version = store.load("ses").unwrap().row_version.unwrap();

    let runner = JoinedUnitRunner::default();
    let pool = TestPool::with_capacity(4 * 1024 * 1024);
    let mut emergency = Box::pin(metered_transform(
        &handler,
        request_with_usage(messages, 48_000, 50_000),
        &runner,
        &pool,
    ));
    poll_fn(|cx| {
        assert!(emergency.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    wait_for_count(&runner.completed, 1).await;
    poll_fn(|cx| {
        assert!(
            emergency.as_mut().poll(cx).is_pending(),
            "the completed first unit must wait for the live history_summarizer"
        );
        Poll::Ready(())
    })
    .await;
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 1);
    assert_eq!(runner.completed.load(Ordering::SeqCst), 1);
    assert!(runner.joins.is_empty());
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert!(pool.try_reserve(pool.capacity()).is_none());
    let committed = store.load("ses").unwrap();
    assert!(committed.row_version.unwrap() > seed_version);
    assert!(
        handler
            .live_history_summarizer_sessions
            .lock()
            .unwrap()
            .contains_key("ses")
    );

    runner.worker.cancel.cancel();
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
    let outcome = watchdog(emergency).await;
    runner.join_all().await;
    handler.tasks.close();
    watchdog(handler.tasks.wait()).await;
    assert_eq!(error_code(outcome), "cancelled");
    let persisted = store.load("ses").unwrap();
    assert!(persisted.row_version >= committed.row_version);
    assert!(persisted.meta.initialized);
    assert_eq!(producer.starts.load(Ordering::SeqCst), 1);
    assert_eq!(runner.submitted.load(Ordering::SeqCst), 2);
    assert_eq!(runner.completed.load(Ordering::SeqCst), 2);
    assert_eq!(handler.transform_units.available_permits(), 4);
    assert_pool_released(&pool);
}

/// Delivers a synthetic join failure without running the failed submission's closure.
struct FailingRunner {
    joined: JoinedUnitRunner,
    fail_on: usize,
    failure: BlockingWorkFailed,
    submissions: AtomicUsize,
}

impl UnitRunner for FailingRunner {
    fn run_unit(
        &self,
        work: Box<dyn FnOnce() -> UnitOutcome + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<UnitOutcome, BlockingWorkFailed>> + Send + 'static>>
    {
        if self.submissions.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_on {
            drop(work);
            Box::pin(std::future::ready(Err(self.failure)))
        } else {
            self.joined.run_unit(work)
        }
    }

    fn run_step(
        &self,
        work: Box<dyn FnOnce() + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlockingWorkFailed>> + Send + 'static>> {
        self.joined.run_step(work)
    }

    fn cancel_signal(&self) -> CancelSignal {
        self.joined.cancel_signal()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn synthetic_unit_failures_map_to_internal_error_and_release_resources() {
    for fail_on in 1..=2 {
        for failure in [
            BlockingWorkFailed::Panicked,
            BlockingWorkFailed::RuntimeStopped,
            BlockingWorkFailed::RouteClosing,
        ] {
            let producer = Arc::new(ProducerState::default());
            let (handler, store, _dir, _project) =
                handler_with_store(Arc::clone(&producer), default_test_config());
            let runner = FailingRunner {
                joined: JoinedUnitRunner::default(),
                fail_on,
                failure,
                submissions: AtomicUsize::new(0),
            };
            let pool = TestPool::with_capacity(4 * 1024 * 1024);
            assert!(store.load("ses").unwrap().row_version.is_none());

            let outcome = watchdog(metered_transform(
                &handler,
                request_with_usage(big_messages(), 48_000, 50_000),
                &runner,
                &pool,
            ))
            .await;
            runner.joined.join_all().await;
            handler.tasks.close();
            watchdog(handler.tasks.wait()).await;

            let (code, _) = error_frame(outcome);
            assert_eq!(
                code, "internal_error",
                "{failure:?} at submission {fail_on} must not map to store_unavailable"
            );
            assert_eq!(runner.submissions.load(Ordering::SeqCst), fail_on);
            assert_eq!(runner.joined.submitted.load(Ordering::SeqCst), fail_on - 1);
            assert_eq!(runner.joined.completed.load(Ordering::SeqCst), fail_on - 1);
            assert!(runner.joined.joins.is_empty());
            assert_eq!(handler.transform_units.available_permits(), 4);
            assert_pool_released(&pool);

            let persisted = store.load("ses").unwrap();
            if fail_on == 1 {
                assert!(persisted.row_version.is_none());
                assert!(store.load_pass_trace("ses").unwrap().is_none());
                assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
            } else {
                assert!(persisted.row_version.is_some());
                assert!(persisted.meta.initialized);
                assert!(
                    persisted
                        .meta
                        .publication_floor_ordinal
                        .is_some_and(|floor| floor > 0)
                );
                assert_eq!(
                    persisted.meta.history_summarizer.state,
                    HistorySummarizerPhase::Idle
                );
                assert_eq!(producer.starts.load(Ordering::SeqCst), 1);
                assert_eq!(producer.await_outputs.load(Ordering::SeqCst), 1);
            }
        }
    }
}
