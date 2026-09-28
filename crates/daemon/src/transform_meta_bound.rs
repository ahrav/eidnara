#![forbid(unsafe_code)]

use std::sync::Arc;

use memory_store::{HistorySummarizerPhase, MemoryStore, MemoryStoreError};

use crate::test_support::synthetic_history::{SyntheticHistory, seed_active_summarizer};
use crate::transform::tests::{item, pctx, req, resolved, store};
use crate::transform::{
    TransformError, TransformRequest, TransformResponse, install_transform_attempt_hook,
    transform_with_projection_cached,
};

const SESSION: &str = "meta-bound";
const WINDOW: u64 = 300;

/// Every test here uses [`SESSION`], and the attempt hook is keyed by session, so a test's
/// pass could consume another test's one-shot hook; the tests run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A store holding `h` synthetic segments over messages 1..=2h and the continuation base the
/// window continues, so an unanchored first pass numbers the window from `2h - 1`.
fn seeded(h: usize) -> (tempfile::TempDir, Arc<MemoryStore>) {
    let dir = tempfile::tempdir().expect("store dir");
    let store = Arc::new(store(dir.path()));
    SyntheticHistory::mixed(h).seed(&store, SESSION);
    let empty = store.load(SESSION).expect("load empty");
    let mut meta = empty.meta.clone();
    meta.ordinal_continuation_base = Some(2 * h as u64 - 2);
    store
        .commit(SESSION, None, &empty.core, &meta)
        .expect("seed the continuation base");
    (dir, store)
}

/// A `null` window of [`WINDOW`] messages: the newest segment's two messages, then the tail.
fn window(h: usize) -> TransformRequest {
    let first = 2 * h as u64 - 1;
    let messages = (first..first + WINDOW)
        .map(|n| item(&format!("m{n}"), n, &format!("message {n} with some text")))
        .collect();
    req(SESSION, "cfg0", messages)
}

fn pass(
    store: &MemoryStore,
    request: &TransformRequest,
) -> Result<TransformResponse, TransformError> {
    let ctx = pctx("git:meta-bound", "/nonexistent-docs", 1_700_000_000_000);
    transform_with_projection_cached(store, &resolved(store, request), &ctx).map(|out| out.response)
}

/// The stored `meta` text's length, as the durable-text bound measures it.
fn meta_row_bytes(store: &MemoryStore) -> usize {
    store
        .with_fenced_conn_for_test(|tx| {
            tx.query_row(
                "SELECT length(CAST(meta AS BLOB)) FROM cache_state WHERE session_id = ?1",
                [SESSION],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("meta length") as usize
}

/// The submitted window's mids, cut prefix included (spec D12).
fn window_mids(request: &TransformRequest) -> Vec<String> {
    let mut mids: Vec<String> = request
        .messages
        .iter()
        .map(|message| message.mid.clone())
        .collect();
    mids.sort();
    mids
}

fn identity_mids(store: &MemoryStore) -> Vec<String> {
    store
        .all_block_identities_for_test(SESSION)
        .into_keys()
        .collect()
}

#[test]
fn a_hundred_thousand_message_session_commits_a_three_hundred_message_window() {
    let _serial = serial();
    let (_dir, store) = seeded(50_000);
    let request = window(50_000);
    let expected = window_mids(&request);
    let first = pass(&store, &request).expect("the window commits");
    assert_eq!(first.action, "HARD");
    assert!(first.committed);
    let after = store.load(SESSION).expect("load after");
    assert_eq!(after.meta.coverage_ordinal, Some(100_000));
    assert_eq!(identity_mids(&store), expected);
    let bytes = meta_row_bytes(&store);
    assert!(
        bytes < 128 * 1024,
        "a 100,000-message session commits {bytes} meta bytes"
    );
}

/// A session written before identities were pruned (WP-P25): a `block_identities` row for every
/// covered message, a `meta` row carrying the embedded identity map that preceded the table
/// near the 512 KiB bound, and a history_summarizer firing awaiting its producer. Returns the
/// store, the next window, and the version the legacy row was written at.
fn legacy_session() -> (tempfile::TempDir, Arc<MemoryStore>, TransformRequest, u64) {
    let h = 1_000;
    let (dir, store) = seeded(h);
    let request = window(h);
    pass(&store, &request).expect("first pass");
    seed_active_summarizer(&store, SESSION);
    let loaded = store.load(SESSION).expect("load");
    let vector = store
        .all_block_identities_for_test(SESSION)
        .into_values()
        .next()
        .unwrap();
    let history = memory_store::BlockIdentityDelta {
        upserts: (1..2 * h as u64 - 1)
            .map(|n| (format!("m{n}"), vector.clone()))
            .collect(),
        ..Default::default()
    };
    let version = store
        .commit_with_block_identities_for_test(
            SESSION,
            loaded.row_version,
            &loaded.core,
            &loaded.meta,
            &history,
        )
        .expect("write every covered identity");
    let entry = serde_json::to_string(&vector).unwrap();
    let embedded = (0..)
        .map(|n| format!("\"legacy-{n}\":{entry}"))
        .scan(0, |bytes, pair| {
            *bytes += pair.len() + 1;
            (*bytes < 500 * 1024 - meta_row_bytes(&store)).then_some(pair)
        })
        .collect::<Vec<_>>()
        .join(",");
    store
        .with_fenced_conn_for_test(|tx| {
            tx.execute(
                "UPDATE cache_state SET meta = json_set(meta, '$.block_identity_by_mid', json(?2))
                  WHERE session_id = ?1",
                rusqlite::params![SESSION, format!("{{{embedded}}}")],
            )
        })
        .expect("embed the legacy map");
    let bytes = meta_row_bytes(&store);
    assert!(
        (480 * 1024..512 * 1024).contains(&bytes),
        "the legacy row holds {bytes} meta bytes"
    );
    let mut next = request;
    let ordinal = 2 * h as u64 - 1 + WINDOW;
    next.messages.push(item(
        &format!("m{ordinal}"),
        ordinal,
        "the message after the legacy window",
    ));
    (dir, store, next, version)
}

fn assert_history_kept(store: &MemoryStore) {
    let loaded = store.load(SESSION).expect("load kept");
    assert_eq!(identity_mids(store).len(), 1_998 + WINDOW as usize + 1);
    assert!(meta_row_bytes(store) < 128 * 1024);
    assert_eq!(
        loaded.meta.history_summarizer.state,
        HistorySummarizerPhase::AwaitingProducer
    );
}

#[test]
fn a_legacy_row_is_read_after_a_restart_and_keeps_its_identity_rows_on_its_first_commit() {
    let _serial = serial();
    let (dir, legacy_store, request, _) = legacy_session();
    drop(legacy_store);
    let store = store(dir.path());
    store.load(SESSION).expect("the legacy row reads");
    let answer = pass(&store, &request).expect("the first pass commits");
    assert!(answer.committed);
    assert_eq!(answer.action, "SOFT+");
    assert_history_kept(&store);
}

#[test]
fn a_pass_that_loses_its_cas_reloads_and_keeps_the_identity_history() {
    let _serial = serial();
    let (_dir, store, request, _) = legacy_session();
    let hook_store = Arc::clone(&store);
    install_transform_attempt_hook(SESSION, move || {
        let loaded = hook_store.load(SESSION).unwrap();
        hook_store
            .commit(SESSION, loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
    });
    let answer = pass(&store, &request).expect("the retry commits");
    assert!(answer.committed);
    assert_history_kept(&store);
}

#[test]
fn a_writer_that_loses_to_a_pass_reloads_the_row() {
    let _serial = serial();
    let (_dir, store, request, version) = legacy_session();
    let stale = store.load(SESSION).unwrap();
    assert_eq!(stale.row_version, Some(version));
    pass(&store, &request).expect("the pass commits");
    assert!(matches!(
        store.commit(SESSION, Some(version), &stale.core, &stale.meta),
        Err(MemoryStoreError::CasConflict { .. })
    ));
    let reloaded = store.load(SESSION).unwrap();
    store
        .commit(
            SESSION,
            reloaded.row_version,
            &reloaded.core,
            &reloaded.meta,
        )
        .expect("the reloaded writer commits");
    assert_history_kept(&store);
}
