//! Pins the durable-text bound of a first HARD pass over a long session: the memory store
//! bounds one durable text field at 512 KiB, so the committed `meta` must not grow with
//! the history that segments already cover.

#![forbid(unsafe_code)]

use crate::canonical_memory::{CanonicalMemoryRead, CanonicalMemorySnapshot};
use crate::config::CacheTtlProvenance;
use crate::transform::{
    ProducerContext, TransformError, TransformRequest, transform_with_projection_cached,
};
use crate::wire::IngressMessage;
use memory_store::{LoadedState, MemoryStore, StoredHistorySegment};

use crate::test_support::transform_corpus::{self as corpus, CORPUS_SEED, ContentClass};

const SESSION: &str = "meta-bound";
/// Messages left uncovered behind the history segments.
const LIVE_TAIL: usize = 200;
const SEGMENT_MESSAGES: usize = 100;

/// Segments of [`SEGMENT_MESSAGES`] messages covering all but the last [`LIVE_TAIL`].
fn covering_segments(count: usize) -> Vec<StoredHistorySegment> {
    (0..(count - LIVE_TAIL) / SEGMENT_MESSAGES)
        .map(|index| {
            let start = index * SEGMENT_MESSAGES + 1;
            let end = start + SEGMENT_MESSAGES - 1;
            StoredHistorySegment {
                sequence: index as i64,
                start_message: start as i64,
                end_message: end as i64,
                end_message_id: format!("m{end}#0"),
                title: format!("C{index}"),
                content: format!("summary of messages {start} through {end}"),
                p1: Some(format!("summary of messages {start} through {end}")),
                importance: 50,
                ..Default::default()
            }
        })
        .collect()
}

fn transform(
    store: &MemoryStore,
    project_directory: &str,
    messages: &[IngressMessage],
) -> Result<String, TransformError> {
    let req: TransformRequest = serde_json::from_value(serde_json::json!({
        "kind": "transform",
        "v": 2,
        "serializer_profile": "owned-llmrunner",
        "session_id": SESSION,
        "render_config": "meta-bound-config",
        "messages": messages,
    }))
    .expect("transform request");
    let ctx = ProducerContext {
        project_memory: Some(CanonicalMemoryRead::Available(
            CanonicalMemorySnapshot::new(0, false, Vec::new()),
        )),
        project_path: "git:meta-bound",
        note_project_path: "git:meta-bound",
        project_directory,
        history_budget_tokens: 60_000.0,
        user_profile_budget_tokens: 4_000.0,
        memory_enabled: true,
        inject_docs: true,
        temporal_awareness: true,
        now_ms: 1_700_000_000_000,
        execute_threshold_percentage: 65.0,
        compaction_enabled: true,
        smart_drops: false,
        cache_ttl: "5m".to_string(),
        cache_ttl_provenance: CacheTtlProvenance::Default,
        model_key: None,
        observed_last_response_at_ms: None,
        guidance_date: Some("Today's date: Thu Jan 01 2026".to_string()),
        history_summarizer_active: false,
        wrapup_active: false,
        injected_reductions: Vec::new(),
    };
    transform_with_projection_cached(store, &req, &ctx, &std::sync::Mutex::default(), None)
        .map(|out| out.response.action)
}

fn meta_bytes(loaded: &LoadedState) -> usize {
    serde_json::to_string(&loaded.meta)
        .expect("meta serializes")
        .len()
}

/// Commits a first HARD pass over `count` messages whose older part is covered, then
/// checks that the covered messages keep their durable identities.
fn first_hard_pass(count: usize) -> (tempfile::TempDir, MemoryStore, Vec<IngressMessage>, usize) {
    let dir = tempfile::tempdir().expect("store dir");
    let store = MemoryStore::open(&crate::store_descriptor_in(dir.path())).expect("store");
    store
        .replace_history_segments(SESSION, &covering_segments(count))
        .expect("seed segments");
    let messages = corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
    let project_directory = dir.path().to_str().expect("utf8 dir").to_string();

    let result = transform(&store, &project_directory, &messages);
    assert!(
        matches!(result.as_deref(), Ok("HARD")),
        "{count} messages must commit: {result:?}"
    );
    let after = store.load(SESSION).expect("load after");
    assert!(
        after.row_version.is_some(),
        "{count}: the HARD pass commits"
    );
    assert_eq!(
        after.meta.coverage_ordinal,
        Some((count - LIVE_TAIL) as u64),
        "{count}: the segments cover all but the live tail"
    );
    assert_eq!(
        after.meta.block_identity_by_mid.len(),
        count,
        "{count}: every message keeps a durable identity"
    );
    let bytes = meta_bytes(&after);
    (dir, store, messages, bytes)
}

#[test]
fn first_hard_pass_meta_respects_the_store_durable_text_bound() {
    for count in [1_400usize, 10_000] {
        let (_dir, _store, _messages, bytes) = first_hard_pass(count);
        assert!(
            bytes < 128 * 1024,
            "{count} messages commit {bytes} meta bytes, far from the 512 KiB bound"
        );
    }
}

#[test]
fn meta_bytes_stay_flat_as_covered_history_grows_and_covered_drift_still_rejects() {
    let (_small_dir, _small_store, _small_messages, small) = first_hard_pass(1_000);
    let (dir, store, mut messages, large) = first_hard_pass(10_000);
    assert!(
        large <= small + small / 10,
        "meta grew from {small} bytes at 1,000 messages to {large} bytes at 10,000"
    );

    // A covered message whose content changes is rejected exactly as before.
    let covered = 4;
    let replacement =
        corpus::messages(ContentClass::Mixed, covered + 1, 2_048, CORPUS_SEED ^ 1).remove(covered);
    assert_eq!(replacement.mid, messages[covered].mid);
    messages[covered] = replacement;
    let before = store.load(SESSION).expect("load before drift");
    let result = transform(&store, dir.path().to_str().expect("utf8 dir"), &messages);
    assert!(
        matches!(&result, Err(TransformError::IdentityDrift(mid)) if mid == "m5"),
        "covered drift must reject: {result:?}"
    );
    let after = store.load(SESSION).expect("load after drift");
    assert_eq!(
        after.row_version, before.row_version,
        "a rejected pass commits nothing"
    );
    assert_eq!(
        after.meta, before.meta,
        "a rejected pass keeps the stored identities"
    );
}
