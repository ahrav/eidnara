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
