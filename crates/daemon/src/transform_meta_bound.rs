//! The durable meta bound under revision 3 (spec C4, D12; WP-E10, WP-P25). The memory store
//! bounds one durable text field at 512 KiB; block identities are window-scoped, so a long
//! session commits its window with its `meta` row and its `block_identities` rows independent
//! of the history the segments cover, and a row written before identities were pruned is read
//! as-is and pruned to the window on its first ordinary commit.

#![forbid(unsafe_code)]

use std::sync::Arc;

use memory_store::{HistorySummarizerPhase, LoadedState, MemoryStore, MemoryStoreError};

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

fn identity_mids(loaded: &LoadedState) -> Vec<String> {
    loaded.meta.block_identity_by_mid.keys().cloned().collect()
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
    assert_eq!(identity_mids(&after), expected);
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
    let vector = loaded
        .meta
        .block_identity_by_mid
        .values()
        .next()
        .unwrap()
        .clone();
    let mut meta = loaded.meta.clone();
    for n in 1..2 * h as u64 - 1 {
        meta.block_identity_by_mid
            .insert(format!("m{n}"), vector.clone());
    }
    let version = store
        .commit(SESSION, loaded.row_version, &loaded.core, &meta)
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
    (dir, store, request, version)
}

/// The first ordinary commit over a legacy row leaves exactly the window's identities and a
/// `meta` row below 128 KiB, with the firing kept. The identity rows are the prune check; the
/// meta size is a bound check, since any struct-serialized commit drops the embedded key.
fn assert_pruned(store: &MemoryStore, request: &TransformRequest) {
    let loaded = store.load(SESSION).expect("load pruned");
    assert_eq!(identity_mids(&loaded), window_mids(request));
    assert!(meta_row_bytes(store) < 128 * 1024);
    assert_eq!(
        loaded.meta.history_summarizer.state,
        HistorySummarizerPhase::AwaitingProducer
    );
}

#[test]
fn a_legacy_row_is_read_after_a_restart_and_pruned_on_its_first_commit() {
    let _serial = serial();
    let (dir, legacy_store, request, _) = legacy_session();
    drop(legacy_store);
    let store = store(dir.path());
    let legacy = store.load(SESSION).expect("the legacy row reads");
    assert_eq!(
        legacy.meta.block_identity_by_mid.len(),
        1_998 + WINDOW as usize
    );
    let answer = pass(&store, &request).expect("the first pass commits");
    assert!(answer.committed);
    // Not a HARD: the prune runs on every ordinary commit (WP-P25).
    assert_eq!(answer.action, "SOFT+");
    assert_pruned(&store, &request);
}

#[test]
fn a_legacy_prune_that_loses_its_cas_reloads_and_prunes() {
    let _serial = serial();
    let (_dir, store, request, _) = legacy_session();
    let hook_store = Arc::clone(&store);
    let conflicting_meta_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recorded = Arc::clone(&conflicting_meta_bytes);
    install_transform_attempt_hook(SESSION, move || {
        let loaded = hook_store.load(SESSION).unwrap();
        hook_store
            .commit(SESSION, loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
        recorded.store(
            meta_row_bytes(&hook_store),
            std::sync::atomic::Ordering::SeqCst,
        );
    });
    let answer = pass(&store, &request).expect("the retry commits");
    assert!(answer.committed);
    // The conflicting writer's own commit already shrinks the meta row; the retry prunes the
    // identity rows it reloads.
    let conflicting = conflicting_meta_bytes.load(std::sync::atomic::Ordering::SeqCst);
    assert!(conflicting > 0 && conflicting < 128 * 1024, "{conflicting}");
    assert_eq!(
        store
            .load(SESSION)
            .unwrap()
            .meta
            .block_identity_by_mid
            .len(),
        WINDOW as usize
    );
    assert_pruned(&store, &request);
}

#[test]
fn a_writer_that_loses_to_the_legacy_prune_reloads_the_pruned_row() {
    let _serial = serial();
    let (_dir, store, request, version) = legacy_session();
    let stale = store.load(SESSION).unwrap();
    assert_eq!(stale.row_version, Some(version));
    pass(&store, &request).expect("the prune commits");
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
    assert_pruned(&store, &request);
}
