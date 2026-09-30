use super::*;
use crate::history_summarizer_archive::{HALF_CAP_BLOCKS, WINDOW_CAP_BLOCKS};

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

async fn pass(handler: &Handler, messages: &[IngressMessage]) -> Value {
    call_transform_request(
        handler,
        request_with_usage(messages.to_vec(), 1_000, 200_000),
    )
    .await
}

async fn wait_for_phase(store: &MemoryStore, phase: HistorySummarizerPhase) {
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    while store.load("ses").unwrap().meta.history_summarizer.state != phase {
        assert!(
            std::time::Instant::now() < deadline,
            "history_summarizer did not reach {phase:?}"
        );
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
}

fn chunk_range(store: &MemoryStore) -> memory_store::HistorySummarizerChunkRange {
    store
        .load("ses")
        .unwrap()
        .meta
        .history_summarizer
        .chunk_range
        .expect("the firing records its chunk")
}

#[tokio::test(flavor = "current_thread")]
async fn a_published_cap_firing_fires_again_only_once_the_uncovered_window_reaches_the_cap() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let last = WINDOW_CAP_BLOCKS as u64 + 100;
    let first = pass(&handler, &window(last)).await;
    assert_eq!(
        first["history_summarizer"]["reason"], "window_cap",
        "{first}"
    );
    let covered = last - HALF_CAP_BLOCKS as u64;
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    while store.max_history_segment_end_ordinal("ses").unwrap() < covered as i64
        || store.load("ses").unwrap().meta.history_summarizer.state != HistorySummarizerPhase::Idle
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the cap firing did not publish through the cut"
        );
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
    assert_eq!(
        store.max_history_segment_end_ordinal("ses").unwrap(),
        covered as i64
    );

    let starts = producer.starts.load(Ordering::SeqCst);
    for grown in [last + 2, covered + WINDOW_CAP_BLOCKS as u64 - 1] {
        let response = pass(&handler, &window(grown)).await;
        assert_ne!(
            response["history_summarizer"]["fired"], true,
            "{grown} messages: {response}"
        );
        assert_eq!(
            producer.starts.load(Ordering::SeqCst),
            starts,
            "{grown} messages start a producer run"
        );
    }

    producer.block_output.store(true, Ordering::SeqCst);
    let rearmed = pass(&handler, &window(covered + WINDOW_CAP_BLOCKS as u64)).await;
    assert_eq!(
        rearmed["history_summarizer"]["reason"], "window_cap",
        "{rearmed}"
    );
    wait_for_phase(&store, HistorySummarizerPhase::AwaitingProducer).await;
    assert_eq!(chunk_range(&store).from_ordinal, covered + 1);
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// The projection retains the cut message after the hook removes its committed identities
/// from the store.
#[tokio::test(flavor = "current_thread")]
async fn the_cap_firing_ends_its_head_on_a_message_with_durable_identities() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
    let cut = messages.len() as u64 - HALF_CAP_BLOCKS as u64;
    {
        let store = Arc::clone(&store);
        *handler
            .between_transform_and_prepare
            .lock()
            .expect("interleave hook mutex") = Some(Box::new(move || {
            let loaded = store.load("ses").unwrap();
            let mid = format!("m{cut}");
            assert!(
                store
                    .all_block_identities_for_test("ses")
                    .contains_key(&mid)
            );
            store
                .commit_with_block_identities_for_test(
                    "ses",
                    loaded.row_version,
                    &loaded.core,
                    &loaded.meta,
                    &memory_store::BlockIdentityDelta {
                        deletes: [mid].into_iter().collect(),
                        ..memory_store::BlockIdentityDelta::default()
                    },
                )
                .unwrap();
        }));
    }

    let first = pass(&handler, &messages).await;
    assert_eq!(
        first["history_summarizer"]["reason"], "window_cap",
        "{first}"
    );
    wait_for_phase(&store, HistorySummarizerPhase::AwaitingProducer).await;
    let range = chunk_range(&store);
    assert_eq!((range.from_ordinal, range.to_ordinal), (1, cut + 1));
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

fn expire_firing_deadline(store: &MemoryStore) {
    let loaded = store.load("ses").unwrap();
    let mut meta = loaded.meta;
    let budget = history_summarizer::completion_wait_budget().as_millis() as i64;
    meta.history_summarizer.fired_at_ms = Some(now_ms() - budget - 1);
    store
        .commit("ses", loaded.row_version, &loaded.core, &meta)
        .unwrap();
}

/// Ordinals of the fixture's original messages that `response` serves, in order.
fn served_ordinals(response: &Value) -> Vec<u64> {
    response["messages"]
        .as_array()
        .expect("the response serves messages")
        .iter()
        .filter_map(|message| {
            message["meta"]["harness_id"]
                .as_str()?
                .strip_prefix('m')?
                .parse()
                .ok()
        })
        .collect()
}

fn rendered(store: &MemoryStore) -> Option<memory_store::HistorySegmentEdge> {
    store.coverage_snapshot("ses", None, &[]).unwrap().rendered
}

async fn stalled_firing_at_the_cap(
    producer: &Arc<ProducerState>,
) -> (
    Handler,
    Arc<MemoryStore>,
    tempfile::TempDir,
    Vec<IngressMessage>,
) {
    producer.block_output.store(true, Ordering::SeqCst);
    let (handler, store, dir, _project) =
        handler_with_store(Arc::clone(producer), default_test_config());
    let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
    let first = pass(&handler, &messages).await;
    assert_eq!(first["history_summarizer"]["fired"], true, "{first}");
    assert_eq!(first["history_summarizer"]["reason"], "window_cap");
    wait_for_phase(&store, HistorySummarizerPhase::AwaitingProducer).await;
    expire_firing_deadline(&store);
    (handler, store, dir, messages)
}

/// An ordinary segment is published and folded first, so the HARD after the archive can only
/// come from the archive: the pass that archives moves no boundary, the next pass folds the
/// archive with a HARD and leaves half the cap, and the next firing starts after the archive.
#[tokio::test(flavor = "current_thread")]
async fn a_stalled_summarizer_archives_the_window_to_half_the_cap_through_the_next_hard() {
    let producer = Arc::new(ProducerState::default());
    producer
        .outputs
        .lock()
        .unwrap()
        .push_back(history_summarizer_output(1, 10, "ordinary arc"));
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
    let ordinary = pass(&handler, &messages).await;
    assert_eq!(
        ordinary["history_summarizer"]["reason"], "window_cap",
        "{ordinary}"
    );
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    while store.load_history_segments("ses").unwrap().is_empty()
        || store.load("ses").unwrap().meta.history_summarizer.state != HistorySummarizerPhase::Idle
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the ordinary segment was not published"
        );
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
    producer.block_output.store(true, Ordering::SeqCst);
    let first_fold = pass(&handler, &messages).await;
    assert_eq!(first_fold["action"], "HARD", "{first_fold}");
    assert_eq!(
        first_fold["history_summarizer"]["reason"], "window_cap",
        "{first_fold}"
    );
    let before = rendered(&store).expect("the ordinary segment is rendered");
    assert_eq!(before.end_message, 10);
    wait_for_phase(&store, HistorySummarizerPhase::AwaitingProducer).await;
    expire_firing_deadline(&store);
    let stalled = store.load("ses").unwrap().meta.history_summarizer;
    let predicate = history_summarizer::publish_predicate(&stalled).unwrap();

    let archiving = pass(&handler, &messages).await;
    assert_eq!(
        archiving["history_summarizer"]["no_fire"], "archived",
        "{archiving}"
    );
    assert_ne!(archiving["action"], "HARD", "{archiving}");
    assert_eq!(
        rendered(&store),
        Some(before),
        "the archive alone moves no boundary"
    );
    let after_archive = store.load("ses").unwrap();
    let summarizer = &after_archive.meta.history_summarizer;
    assert_eq!(summarizer.state, HistorySummarizerPhase::Idle);
    assert_eq!(
        summarizer
            .last_abandon
            .as_ref()
            .map(|abandon| (abandon.firing_seq, abandon.reason)),
        Some((
            stalled.firing_seq,
            memory_store::HistorySummarizerAbandonReason::WindowArchived
        ))
    );
    let segments = store.load_history_segments("ses").unwrap();
    assert_eq!(segments.len(), 2);
    let archive = segments.last().unwrap();
    assert_eq!(
        archive.episode_type.as_deref(),
        Some(memory_store::ARCHIVE_EPISODE_TYPE)
    );
    assert_eq!(archive.legacy, 0);
    let end = messages.len() as u64 - HALF_CAP_BLOCKS as u64;
    assert_eq!(
        (archive.start_message, archive.end_message),
        (11, end as i64)
    );
    assert_eq!(archive.end_message_id, format!("m{end}#0"));
    assert_eq!(after_archive.meta.archive_fold_seq, Some(archive.sequence));
    assert_eq!(after_archive.meta.publication_floor_ordinal, Some(end + 1));

    let stale =
        store.publish_history_summarizer_chunk(memory_store::HistorySummarizerPublishRequest {
            session_id: "ses",
            expected_row_version: after_archive.row_version,
            expected_revert_epoch: stalled.expected_revert_epoch,
            predicate: &predicate,
            project_path: "git:proj",
            history_segments: &[stored_comp(0, 11, 13, "m13", "stale summary")],
            events: &[],
            primer_candidates: &[],
            user_memory_candidates: &[],
            publication_floor_ordinal: 14,
            chunk_transcript: None,
            memory_reviewer_nonadmission: None,
            memory_reviewer_activation: None,
            published_at_ms: 0,
        });
    assert!(
        matches!(
            stale,
            Err(memory_store::HistorySummarizerPublishError::InvalidState { .. })
        ),
        "{stale:?}"
    );

    let folding = pass(&handler, &messages).await;
    assert_eq!(folding["action"], "HARD", "{folding}");
    assert_eq!(folding["materialize_reason"], "coverage_fold", "{folding}");
    let boundary = rendered(&store).expect("the HARD renders the archive's end");
    assert_eq!(
        (boundary.sequence, boundary.end_message),
        (archive.sequence, archive.end_message)
    );
    let served = served_ordinals(&folding);
    assert_eq!(
        served,
        (end + 1..=messages.len() as u64).collect::<Vec<_>>(),
        "the HARD serves every message after the archive's end"
    );
    assert_eq!(served.len(), HALF_CAP_BLOCKS);
    assert_eq!(store.load_history_segments("ses").unwrap().len(), 2);
    assert!(
        store
            .load("ses")
            .unwrap()
            .meta
            .history_summarizer
            .last_abandon
            .is_some()
    );

    let grown = window(end + WINDOW_CAP_BLOCKS as u64);
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    loop {
        let response = pass(&handler, &grown).await;
        if response["history_summarizer"]["fired"] == true {
            break;
        }
        assert_eq!(
            response["history_summarizer"]["no_fire"], "busy",
            "{response}"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "no firing followed the archive"
        );
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
    let next = store.load("ses").unwrap().meta.history_summarizer;
    assert!(next.firing_seq > stalled.firing_seq);
    assert_eq!(
        next.chunk_range.map(|range| range.from_ordinal),
        Some(end + 1)
    );
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// An idle summarizer in failure backoff cannot fire, so a capped window is archived without
/// ending a firing and the next pass folds it with a HARD.
#[tokio::test(flavor = "current_thread")]
async fn an_idle_summarizer_in_backoff_archives_the_capped_window() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let quiet = pass(&handler, &window(10)).await;
    assert_ne!(quiet["history_summarizer"]["fired"], true, "{quiet}");
    let loaded = store.load("ses").unwrap();
    let mut meta = loaded.meta;
    meta.history_summarizer.failure_backoff_at_ms = Some(now_ms() + 3_600_000);
    store
        .commit("ses", loaded.row_version, &loaded.core, &meta)
        .unwrap();

    let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
    let archiving = pass(&handler, &messages).await;
    assert_eq!(
        archiving["history_summarizer"]["no_fire"], "archived",
        "{archiving}"
    );
    let end = messages.len() as u64 - HALF_CAP_BLOCKS as u64;
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
        [(1, end as i64, true)]
    );
    let summarizer = store.load("ses").unwrap().meta.history_summarizer;
    assert_eq!(summarizer.state, HistorySummarizerPhase::Idle);
    assert_eq!(summarizer.last_abandon, None);

    let folding = pass(&handler, &messages).await;
    assert_eq!(folding["action"], "HARD", "{folding}");
    assert_eq!(
        served_ordinals(&folding),
        (end + 1..=messages.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(
        store.load("ses").unwrap().meta.history_summarizer.state,
        HistorySummarizerPhase::Idle,
        "no firing starts while the backoff holds"
    );
}

/// A fallback chain persists an idle state in backoff before it closes the rejected attempt and
/// fires its next model. An archive in that gap ends the chain, so the first model's run is the
/// only one.
#[tokio::test(flavor = "current_thread")]
async fn an_archive_between_fallback_models_ends_the_chain_before_its_next_run() {
    let producer = Arc::new(ProducerState::default());
    producer.outputs.lock().unwrap().push_back(
        history_summarizer_output(1, 10, "rejected arc").replace(
            r#"episode_type="feature""#,
            &format!(r#"episode_type="{}""#, memory_store::ARCHIVE_EPISODE_TYPE),
        ),
    );
    producer.block_close_attempt.store(true, Ordering::SeqCst);
    let config = DaemonConfig {
        model_chain: vec!["test/model".to_string(), "test/fallback".to_string()],
        ..default_test_config()
    };
    let (handler, store, _dir, _project) = handler_with_store(Arc::clone(&producer), config);
    let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
    let first = pass(&handler, &messages).await;
    assert_eq!(
        first["history_summarizer"]["reason"], "window_cap",
        "{first}"
    );
    wait_for_count(&producer.close_attempts, 1).await;
    let rejected = store.load("ses").unwrap().meta.history_summarizer;
    assert_eq!(rejected.state, HistorySummarizerPhase::Idle);
    assert!(
        rejected
            .failure_backoff_at_ms
            .is_some_and(|backoff_at_ms| backoff_at_ms > now_ms()),
        "{rejected:?}"
    );
    assert_eq!(producer.starts.load(Ordering::SeqCst), 1);

    let archiving = pass(&handler, &messages).await;
    assert_eq!(
        archiving["history_summarizer"]["no_fire"], "archived",
        "{archiving}"
    );

    producer.block_close_attempt.store(false, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    while handler
        .live_history_summarizer_sessions
        .lock()
        .unwrap()
        .contains_key("ses")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the fallback chain did not end"
        );
        producer.notify.notify_waiters();
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
    assert_eq!(
        producer.starts.load(Ordering::SeqCst),
        1,
        "the first model's run is the only one"
    );
    let after = store.load("ses").unwrap().meta.history_summarizer;
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.firing_seq, rejected.firing_seq);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publication_that_commits_before_the_archive_leaves_the_archive_refused() {
    let producer = Arc::new(ProducerState::default());
    producer
        .outputs
        .lock()
        .unwrap()
        .push_back(history_summarizer_output(
            1,
            3,
            "published before the archive",
        ));
    let (handler, store, _dir, messages) = stalled_firing_at_the_cap(&producer).await;
    let hook_producer = Arc::clone(&producer);
    store.set_before_history_archive_hook(Box::new(move |store| {
        hook_producer.block_output.store(false, Ordering::SeqCst);
        hook_producer.notify.notify_waiters();
        let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
        while store.load_history_segments("ses").unwrap().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "the firing did not publish"
            );
            std::thread::sleep(TEST_WAIT_POLL);
        }
    }));

    let response = pass(&handler, &messages).await;
    assert_ne!(
        response["history_summarizer"]["no_fire"], "archived",
        "{response}"
    );
    let loaded = store.load("ses").unwrap();
    let segments = store.load_history_segments("ses").unwrap();
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.start_message, segment.end_message))
            .collect::<Vec<_>>(),
        [(1, 3)]
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment.episode_type.as_deref()
                != Some(memory_store::ARCHIVE_EPISODE_TYPE))
    );
    assert_eq!(loaded.meta.archive_fold_seq, None);
    assert_eq!(loaded.meta.history_summarizer.last_abandon, None);
}

/// At the cap the firing's eligible head reaches through the half-cap cut, so its chunk
/// starts at the window's first message and ends no earlier than the cut.
#[tokio::test(flavor = "current_thread")]
async fn a_window_at_the_cap_fires_a_chunk_that_reaches_the_half_cap_cut() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let below = window(WINDOW_CAP_BLOCKS as u64 - 1);
    let quiet = pass(&handler, &below).await;
    assert_ne!(
        quiet["history_summarizer"]["reason"], "window_cap",
        "{quiet}"
    );

    let messages = window(WINDOW_CAP_BLOCKS as u64 + 100);
    let first = pass(&handler, &messages).await;
    assert_eq!(first["history_summarizer"]["fired"], true, "{first}");
    assert_eq!(
        first["history_summarizer"]["reason"], "window_cap",
        "{first}"
    );
    wait_for_phase(&store, HistorySummarizerPhase::AwaitingProducer).await;
    let cut = messages.len() as u64 - HALF_CAP_BLOCKS as u64;
    let range = store
        .load("ses")
        .unwrap()
        .meta
        .history_summarizer
        .chunk_range
        .expect("the firing records its chunk");
    assert_eq!(range.from_ordinal, 1);
    assert!(
        range.to_ordinal >= cut,
        "{range:?} ends before the cut {cut}"
    );
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// Meta at the cap with a cap-triggered firing in flight, for one and five text blocks per
/// message under production-length message ids, stays within 384 KiB (SB-P47).
#[tokio::test(flavor = "current_thread")]
async fn meta_at_the_cap_with_a_firing_in_flight_stays_within_384_kib() {
    const META_BOUND_BYTES: i64 = 384 * 1024;
    for blocks_per_message in [1_usize, 5] {
        let producer = Arc::new(ProducerState::default());
        producer.block_output.store(true, Ordering::SeqCst);
        let (handler, store, _dir, _project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        let message = |ordinal: u64| {
            let texts: Vec<String> = (0..blocks_per_message)
                .map(|block| format!("message {ordinal} block {block}"))
                .collect();
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            let role = if ordinal.is_multiple_of(2) {
                "assistant"
            } else {
                "user"
            };
            crate::transform::tests::wire_item(
                role,
                &format!("msg_01J9ZQ8F3K7M2P5R6T{ordinal:010}"),
                ordinal,
                &texts,
            )
        };
        let at_cap = (WINDOW_CAP_BLOCKS / blocks_per_message) as u64;
        let mut messages: Vec<IngressMessage> = (1..=at_cap).map(message).collect();
        let first = pass(&handler, &messages).await;
        assert_eq!(
            first["history_summarizer"]["reason"], "window_cap",
            "{first}"
        );
        wait_for_phase(&store, HistorySummarizerPhase::AwaitingProducer).await;
        messages.push(message(at_cap + 1));
        let committed = pass(&handler, &messages).await;
        assert_eq!(committed["committed"], true, "{committed}");
        let loaded = store.load("ses").unwrap();
        assert!(
            !loaded
                .meta
                .history_summarizer
                .selected_range_identities
                .is_empty()
        );
        let bytes: i64 = store
            .with_fenced_conn_for_test(|tx| {
                tx.query_row(
                    "SELECT length(CAST(meta AS BLOB)) FROM cache_state WHERE session_id = 'ses'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert!(
            bytes <= META_BOUND_BYTES,
            "{blocks_per_message} blocks per message: meta {bytes} bytes"
        );
        producer.block_output.store(false, Ordering::SeqCst);
        producer.notify.notify_waiters();
    }
}
