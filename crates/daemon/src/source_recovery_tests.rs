use super::*;
use crate::history_summarizer_producer::{ErrorClass, ErrorClassification};
use host_runtime::model_execution::protocol::ErrorScope;

const SOURCE_RETRY_SECS: u64 = 120;

fn source_failure() -> HistorySummarizerProducerError {
    HistorySummarizerProducerError::RunFailed {
        run_id: "run-source".to_owned(),
        detail: "opencode AWS credential source is cooling down after a failed refresh".to_owned(),
        classification: Some(ErrorClassification {
            class: ErrorClass::Transient,
            retry_after_secs: Some(SOURCE_RETRY_SECS),
            scope: ErrorScope::CredentialSource,
        }),
        class_field_present: true,
    }
}

fn fail_next_run_on_the_source(producer: &ProducerState) {
    producer
        .await_results
        .lock()
        .unwrap()
        .push_back(Err(source_failure()));
}

fn assert_the_source_failure_advanced_nothing(
    store: &MemoryStore,
    before: &memory_store::LoadedState,
) {
    let loaded = store.load("ses").unwrap();
    assert!(store.load_history_segments("ses").unwrap().is_empty());
    let summarizer = &loaded.meta.history_summarizer;
    assert_eq!(summarizer.chunk_retry, None);
    assert_eq!(loaded.meta.coverage_ordinal, before.meta.coverage_ordinal);
    assert!(
        summarizer
            .failure_backoff_at_ms
            .is_some_and(|at| at >= now_ms() + (SOURCE_RETRY_SECS as i64 - 5) * 1_000),
        "{summarizer:?}"
    );
    assert!(summarizer.last_failure.is_some());
}

fn assert_a_model_fold_published(store: &MemoryStore) {
    let segments = store.load_history_segments("ses").unwrap();
    assert!(!segments.is_empty(), "a model fold published");
    assert!(
        segments
            .iter()
            .all(|segment| !segment.is_archive()
                && !segment.title.starts_with("Unsummarized messages")),
        "{segments:?}"
    );
    assert_eq!(
        store
            .load("ses")
            .unwrap()
            .meta
            .history_summarizer
            .chunk_retry,
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_normal_path_folds_again_after_a_source_failure_and_cooldown() {
    let producer = Arc::new(ProducerState::default());
    fail_next_run_on_the_source(&producer);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = big_messages();
    let _ = call_transform_request(&handler, request(vec![ck("m1", 1, "warm")])).await;
    let before = store.load("ses").unwrap();

    let response = call_transform(&handler, messages.clone()).await;
    assert_eq!(response["history_summarizer"]["fired"], true, "{response}");
    wait_for_count(&producer.starts, 1).await;
    wait_for_idle(&store).await;
    assert_the_source_failure_advanced_nothing(&store, &before);

    let held = call_transform(&handler, messages.clone()).await;
    assert_eq!(held["history_summarizer"]["no_fire"], "backoff", "{held}");
    assert_eq!(producer.starts.load(Ordering::SeqCst), 1);

    fire_and_settle(&handler, &store, &messages).await;
    assert_a_model_fold_published(&store);
    assert_eq!(producer.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn the_emergency_path_folds_again_after_a_source_failure_and_cooldown() {
    let producer = Arc::new(ProducerState::default());
    fail_next_run_on_the_source(&producer);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = big_messages();
    let _ = call_transform_request(&handler, request(vec![ck("m1", 1, "warm")])).await;
    let before = store.load("ses").unwrap();

    let response = call_transform_with_usage(&handler, messages.clone(), 48_000, 50_000).await;
    assert_eq!(response["history_summarizer"]["fired"], true, "{response}");
    wait_for_count(&producer.starts, 1).await;
    wait_for_idle(&store).await;
    assert_the_source_failure_advanced_nothing(&store, &before);

    expire_history_summarizer_backoff(&store);
    tokio::time::timeout(TEST_WAIT_BUDGET, async {
        let response = call_transform_with_usage(&handler, messages, 48_000, 50_000).await;
        assert_eq!(response["history_summarizer"]["fired"], true, "{response}");
        wait_for_count(&producer.starts, 2).await;
    })
    .await
    .expect("the recovered emergency firing started within the wait budget");
    wait_for_history_summarizer_state(&store, |state| {
        state.state == HistorySummarizerPhase::Idle && state.firing_seq >= 2
    })
    .await;
    assert_a_model_fold_published(&store);
}

#[tokio::test(flavor = "current_thread")]
async fn the_wrapup_path_folds_again_after_a_source_failure_and_cooldown() {
    let producer = Arc::new(ProducerState::default());
    fail_next_run_on_the_source(&producer);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    cache_wrapup_messages(&handler, wrapup_messages(80, 800));
    let before = store.load("ses").unwrap();
    let wrapup = || async {
        tool_body(
            handler
                .dispatch_value(
                    test_route(7),
                    json!({ "method": "session.wrapup", "v": 1, "session_id": "ses" }),
                )
                .await,
        )
    };

    let failed = wrapup().await;
    assert_eq!(failed["ok"], json!(false), "{failed}");
    assert_eq!(failed["disposition"], json!("retryable"), "{failed}");
    assert_the_source_failure_advanced_nothing(&store, &before);

    producer.outputs.lock().unwrap().clear();
    expire_history_summarizer_backoff(&store);
    let folded = tokio::time::timeout(TEST_WAIT_BUDGET, wrapup())
        .await
        .expect("the recovered wrapup folds within the wait budget");
    assert_eq!(folded["ok"], json!(true), "{folded}");
    assert_a_model_fold_published(&store);
}

#[tokio::test(flavor = "current_thread")]
async fn the_reattach_path_folds_again_after_a_source_failure_and_cooldown() {
    let producer = Arc::new(ProducerState::default());
    fail_next_run_on_the_source(&producer);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = big_messages();
    let _ = call_transform_request(&handler, request(vec![ck("m1", 1, "warm")])).await;
    let before = store.load("ses").unwrap();
    seed_awaiting(&store, &messages);

    let response = call_transform(&handler, messages.clone()).await;
    assert_eq!(
        response["history_summarizer"]["no_fire"], "reattaching",
        "{response}"
    );
    wait_for_idle(&store).await;
    assert_the_source_failure_advanced_nothing(&store, &before);
    assert_eq!(
        producer.starts.load(Ordering::SeqCst),
        0,
        "reattach starts no model"
    );

    producer.outputs.lock().unwrap().clear();
    fire_and_settle(&handler, &store, &messages).await;
    assert_a_model_fold_published(&store);
}

fn window(last: u64) -> Vec<IngressMessage> {
    (1..=last)
        .map(|ordinal| {
            ck(
                &format!("m{ordinal}"),
                ordinal,
                &format!("message {ordinal}"),
            )
        })
        .collect()
}

async fn small_pass(handler: &Handler, messages: &[IngressMessage]) -> Value {
    call_transform_request(
        handler,
        request_with_usage(messages.to_vec(), 1_000, 200_000),
    )
    .await
}

fn set_firing_deadline(store: &MemoryStore, at_ms: i64) {
    let loaded = store.load("ses").unwrap();
    let mut meta = loaded.meta;
    meta.history_summarizer.fired_at_ms = Some(at_ms);
    store
        .commit("ses", loaded.row_version, &loaded.core, &meta)
        .unwrap();
}

async fn usage_pass(handler: &Handler, messages: &[IngressMessage], usage: (u64, u64)) -> Value {
    call_transform_request(
        handler,
        request_with_usage(messages.to_vec(), usage.0, usage.1),
    )
    .await
}

async fn outage_below_the_cap(usage: (u64, u64)) {
    use crate::history_summarizer_archive::WINDOW_CAP_BLOCKS;
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let below = window(WINDOW_CAP_BLOCKS as u64 - 10);
    let _ = small_pass(&handler, &window(10)).await;
    let before = store.load("ses").unwrap();

    fail_next_run_on_the_source(&producer);
    let fired = usage_pass(&handler, &below, usage).await;
    assert_eq!(
        fired["history_summarizer"]["fired"], true,
        "{usage:?}: {fired}"
    );
    wait_for_idle(&store).await;
    assert_the_source_failure_advanced_nothing(&store, &before);
    for _ in 0..3 {
        let held = usage_pass(&handler, &below, usage).await;
        assert_eq!(
            held["history_summarizer"]["no_fire"], "backoff",
            "{usage:?}: {held}"
        );
    }
    assert_the_source_failure_advanced_nothing(&store, &before);

    seed_awaiting(&store, &below);
    producer.block_output.store(true, Ordering::SeqCst);
    set_firing_deadline(&store, 1);
    let held = usage_pass(&handler, &below, usage).await;
    assert_eq!(
        held["history_summarizer"]["no_fire"], "reattaching",
        "{usage:?}: {held}"
    );
    fail_next_run_on_the_source(&producer);
    producer.block_output.store(false, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    while store.load("ses").unwrap().meta.history_summarizer.state != HistorySummarizerPhase::Idle {
        assert!(
            std::time::Instant::now() < deadline,
            "the reattach did not settle"
        );
        producer.notify.notify_waiters();
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
    assert_the_source_failure_advanced_nothing(&store, &before);

    producer.outputs.lock().unwrap().clear();
    expire_history_summarizer_backoff(&store);
    let fired = usage_pass(&handler, &below, usage).await;
    assert_eq!(
        fired["history_summarizer"]["fired"], true,
        "{usage:?}: {fired}"
    );
    wait_for_history_summarizer_state(&store, |state| {
        state.state == HistorySummarizerPhase::Idle && state.last_failure.is_none()
    })
    .await;
    assert_a_model_fold_published(&store);
}

#[tokio::test(flavor = "current_thread")]
async fn a_normal_path_source_outage_below_the_window_cap_advances_nothing_then_folds() {
    outage_below_the_cap((1_000, 200_000)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_emergency_path_source_outage_below_the_window_cap_advances_nothing_then_folds() {
    outage_below_the_cap((48_000, 50_000)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn at_the_window_cap_a_source_backoff_archives_like_a_model_backoff() {
    use crate::history_summarizer_archive::{HALF_CAP_BLOCKS, WINDOW_CAP_BLOCKS};
    for last_failure in [
        format!("producer: {}", source_failure()),
        "producer: run run-1 failed: model overloaded".to_owned(),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (handler, store, _dir, _project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        let _ = small_pass(&handler, &window(10)).await;
        let loaded = store.load("ses").unwrap();
        let mut meta = loaded.meta;
        meta.history_summarizer.failure_backoff_at_ms = Some(now_ms() + 3_600_000);
        meta.history_summarizer.last_failure = Some(last_failure.clone());
        store
            .commit("ses", loaded.row_version, &loaded.core, &meta)
            .unwrap();
        let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
        let archiving = small_pass(&handler, &messages).await;
        assert_eq!(
            archiving["history_summarizer"]["no_fire"], "archived",
            "{last_failure}: {archiving}"
        );
        let end = messages.len() as i64 - HALF_CAP_BLOCKS as i64;
        let segments = store.load_history_segments("ses").unwrap();
        assert_eq!(
            segments
                .iter()
                .map(|segment| (
                    segment.start_message,
                    segment.end_message,
                    segment.is_archive()
                ))
                .collect::<Vec<_>>(),
            [(1, end, true)],
            "{last_failure}"
        );
        assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn retained_and_local_recovery_need_no_source_while_it_fails() {
    let producer = Arc::new(ProducerState::default());
    producer
        .start_errors
        .lock()
        .unwrap()
        .push_back(Err(source_failure()));
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = big_messages();
    seed_awaiting(&store, &messages);
    let response = call_transform(&handler, messages.clone()).await;
    assert_eq!(
        response["history_summarizer"]["no_fire"], "reattaching",
        "{response}"
    );
    wait_for_idle(&store).await;
    assert!(
        !store.load_history_segments("ses").unwrap().is_empty(),
        "a retained result publishes"
    );
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0, "no model starts");

    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    seed_history_summarizer_phase(&store, HistorySummarizerPhase::Publishing);
    let recovering = call_transform(&handler, messages).await;
    assert_eq!(
        recovering["history_summarizer"]["no_fire"], "recovering",
        "{recovering}"
    );
    wait_for_idle(&store).await;
    assert_eq!(
        producer.starts.load(Ordering::SeqCst),
        0,
        "local recovery starts no model"
    );
    assert_eq!(
        producer.start_errors.lock().unwrap().len(),
        1,
        "the failing source was never consulted"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_reattached_source_failure_honors_its_retry_for_every_class() {
    for class in [ErrorClass::AuthRequired, ErrorClass::Permanent] {
        let producer = Arc::new(ProducerState::default());
        producer.await_results.lock().unwrap().push_back(Err(
            HistorySummarizerProducerError::RunFailed {
                run_id: "run-reattach".to_owned(),
                detail: "opencode AWS credential source needs a new `aws sso login`".to_owned(),
                classification: Some(ErrorClassification {
                    class,
                    retry_after_secs: Some(SOURCE_RETRY_SECS),
                    scope: ErrorScope::CredentialSource,
                }),
                class_field_present: true,
            },
        ));
        let (handler, store, _dir, _project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        let messages = big_messages();
        seed_awaiting(&store, &messages);
        let response = call_transform(&handler, messages).await;
        assert_eq!(
            response["history_summarizer"]["no_fire"], "reattaching",
            "{response}"
        );
        wait_for_idle(&store).await;
        let backoff = store
            .load("ses")
            .unwrap()
            .meta
            .history_summarizer
            .failure_backoff_at_ms;
        assert!(
            backoff.is_some_and(|at| at >= now_ms() + (SOURCE_RETRY_SECS as i64 - 5) * 1_000),
            "{class:?}: {backoff:?}"
        );
    }
}
