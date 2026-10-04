//! Transform revision 3 at the handler seam against a real store: wire admissibility (WP-P24),
//! a duplicate id inside the window (WP-P01), one pass per resolution outcome with its
//! resolution, ordinals, cut, output, and durable effects (WP-P02, WP-P03, WP-P05), the reset of
//! a revert before the first anchor (spec D10), and the interrupted revert under an injected CAS
//! conflict and a panic plus reopen.

use super::revision_goldens::call;
use super::*;
use crate::transform::install_transform_attempt_hook;
use crate::window_coverage::Resolution;

thread_local! {
    /// The session this test thread drives; the interrupted-revert tests take their own, since
    /// the transform attempt hook is keyed by session across the whole test binary.
    static SESSION_ID: std::cell::Cell<&'static str> = const { std::cell::Cell::new("rev3") };
}

fn session() -> &'static str {
    SESSION_ID.with(std::cell::Cell::get)
}

/// A handler over a fresh store bound to `session`, with the clock-dependent m0 inputs off.
fn handler_for(session: &'static str) -> (Handler, Arc<MemoryStore>, tempfile::TempDir) {
    SESSION_ID.with(|id| id.set(session));
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::create_dir_all(dir.path().join("project")).unwrap();
    let (handler, store) = reopened(&dir);
    (handler, store, dir)
}

fn user(mid: &str) -> Value {
    json!({
        "mid": mid,
        "ck": {
            "role": "user",
            "content": [{ "kind": { "type": "text", "text": format!("text of {mid}") } }],
            "meta": { "harness_id": mid }
        }
    })
}

/// A revision 3 body: no ordinal anywhere, `boundary` as given.
fn body(mids: &[&str], boundary: Value) -> Value {
    json!({
        "kind": "transform",
        "v": 3,
        "boundary": boundary,
        "serializer_profile": "owned-llmrunner",
        "session_id": session(),
        "render_config": "cfg0",
        "messages": mids.iter().map(|mid| user(mid)).collect::<Vec<_>>(),
    })
}

fn anchor(mid: &str, sequence: i64) -> Value {
    json!({ "mid": mid, "sequence": sequence })
}

fn segment(sequence: i64, start: i64, end: i64) -> StoredHistorySegment {
    StoredHistorySegment {
        sequence,
        start_message: start,
        end_message: end,
        start_message_id: format!("m{start}#0"),
        end_message_id: format!("m{end}#0"),
        title: format!("S{sequence}"),
        content: format!("summary {sequence}"),
        p1: Some(format!("summary {sequence}")),
        importance: 50,
        ..Default::default()
    }
}

fn mids(range: std::ops::RangeInclusive<u32>) -> Vec<String> {
    range.map(|n| format!("m{n}")).collect()
}

/// The resolution the pass will run under, read before it runs.
fn resolution(store: &MemoryStore, body: &Value) -> crate::window_coverage::Resolved {
    let mut request: TransformRequest = serde_json::from_value(body.clone()).unwrap();
    transform::resolve_window(store, &mut request).unwrap();
    request.coverage.unwrap().resolved.clone()
}

/// Everything a refused or declined pass must leave alone.
fn durable(store: &MemoryStore) -> (Option<u64>, ModuleMeta, Vec<StoredHistorySegment>) {
    let loaded = store.load(session()).unwrap();
    (
        loaded.row_version,
        loaded.meta,
        store.load_history_segments(session()).unwrap(),
    )
}

/// The mids the durable block identities name, sorted.
fn identity_mids(store: &MemoryStore) -> Vec<String> {
    let mut mids: Vec<String> = store
        .all_block_identities_for_test(session())
        .into_keys()
        .collect();
    mids.sort();
    mids
}

/// The recipe operations that keep from the previous output.
fn previous_keeps(response: &Value) -> Vec<Value> {
    response["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["source"] == "previous")
        .cloned()
        .collect()
}

fn served_mids(response: &Value) -> Vec<String> {
    response["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["meta"]["synthetic"] != true)
        .map(|message| message["meta"]["harness_id"].as_str().unwrap().to_string())
        .collect()
}

async fn raw(handler: &Handler, body: Value) -> PreparedOutcome {
    handler.handle_transform_for_test(test_route(7), body).await
}

fn with_base(mut body: Value) -> Value {
    body["base_revision"] = json!("r3-base");
    body
}

/// A session whose first pass folded segments 1 (m1..m2) and 2 (m3..m4) over m1..m6.
async fn folded() -> (Handler, Arc<MemoryStore>, tempfile::TempDir) {
    folded_as("rev3").await
}

/// [`folded`] under its own session, for a test that installs the session-keyed attempt hook.
async fn folded_as(id: &'static str) -> (Handler, Arc<MemoryStore>, tempfile::TempDir) {
    let (handler, store, dir) = handler_for(id);
    store
        .replace_history_segments(session(), &[segment(1, 1, 2), segment(2, 3, 4)])
        .unwrap();
    let names = mids(1..=6);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let first = call(&handler, body(&names, Value::Null)).await;
    assert_eq!(first["action"], "HARD", "{first}");
    (handler, store, dir)
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_or_non_3_revision_is_refused_with_expected_and_received_and_no_state_change() {
    let (handler, store, _dir) = folded().await;
    let before = durable(&store);
    let window = body(&["m4", "m5", "m6"], anchor("m4", 2));
    let mut revision_2 = window.clone();
    revision_2["v"] = json!(2);
    revision_2.as_object_mut().unwrap().remove("boundary");
    for (index, message) in revision_2["messages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        message["ordinal"] = json!(index + 4);
    }
    let mut missing = window.clone();
    missing.as_object_mut().unwrap().remove("v");
    let mut cases = vec![(revision_2, "2"), (missing, "null")];
    for (v, received) in [(json!("3"), "\"3\""), (json!(3.0), "3.0"), (json!(4), "4")] {
        let mut case = window.clone();
        case["v"] = v;
        cases.push((case, received));
    }
    for (case, received) in cases {
        let (code, message) = error_frame(raw(&handler, with_base(case)).await);
        assert_eq!(code, "transform_revision_unsupported");
        assert_eq!(
            message,
            format!("expected transform revision 3, received {received}")
        );
    }
    assert_eq!(durable(&store), before);
}

#[tokio::test(flavor = "current_thread")]
async fn boundary_presence_head_sequence_and_duplicates_are_invalid_params() {
    let (handler, store, _dir) = folded().await;
    let before = durable(&store);
    let mut missing = body(&["m4", "m5"], Value::Null);
    missing.as_object_mut().unwrap().remove("boundary");
    let max_safe = (1i64 << 53) - 1;
    let cases = [
        ("missing boundary", missing),
        ("head is not the mid", body(&["m5", "m6"], anchor("m4", 2))),
        (
            "sequence above 2^53-1",
            body(&["m4"], anchor("m4", max_safe + 1)),
        ),
        (
            "sequence below -(2^53-1)",
            body(&["m4"], anchor("m4", -max_safe - 1)),
        ),
        ("duplicate mid", body(&["m4", "m5", "m5"], anchor("m4", 2))),
        ("declared newer than rendered", {
            store
                .append_history_segments(session(), &[segment(3, 5, 5)])
                .unwrap();
            body(&["m5", "m6"], anchor("m5", 3))
        }),
    ];
    for (name, case) in cases {
        let (code, _) = error_frame(raw(&handler, with_base(case)).await);
        assert_eq!(code, "invalid_params", "{name}");
    }
    let (row_version, meta, _) = durable(&store);
    assert_eq!((row_version, meta), (before.0, before.1));
    // The largest safe sequence is admitted; naming no row, it is `boundary_unknown`.
    let at_limit = call(&handler, body(&["m4"], anchor("m4", max_safe))).await;
    assert_eq!(at_limit["status"], "boundary_unknown");
}

/// A `boundary` that fails typed decoding is `bad_request`, whatever its `v`, and changes nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_boundary_that_does_not_decode_is_bad_request() {
    let (handler, store, _dir) = folded().await;
    let before = durable(&store);
    for boundary in [
        json!({ "sequence": 2 }),
        json!({ "mid": "m4" }),
        json!(5),
        json!({ "mid": "m4", "sequence": 1.5 }),
        json!({ "mid": "m4", "sequence": "2" }),
        json!({ "mid": "m4", "sequence": 9_223_372_036_854_775_808_u64 }),
    ] {
        let (code, _) =
            error_frame(raw(&handler, with_base(body(&["m4"], boundary.clone()))).await);
        assert_eq!(code, "bad_request", "{boundary}");
    }
    let mut revision_2 = body(&["m4"], json!({ "mid": "m4" }));
    revision_2["v"] = json!(2);
    let (code, _) = error_frame(raw(&handler, with_base(revision_2)).await);
    assert_eq!(code, "bad_request", "decoding precedes the revision check");
    assert_eq!(durable(&store), before);
}

/// A page the envelope splits: every page carries a message, the final page the scalars.
fn pages(whole: &Value) -> Vec<Value> {
    let messages = whole["messages"].as_array().unwrap();
    let total = messages.len();
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let mut page = if index + 1 == total {
                whole.clone()
            } else {
                json!({ "method": "transform", "session_id": session() })
            };
            page["messages"] = json!([message]);
            page["transform_page_id"] = json!("r3-pages");
            page["transform_generation"] = json!(1);
            page["transform_page_index"] = json!(index);
            page["transform_page_total"] = json!(total);
            page["transform_page_complete"] = json!(index + 1 == total);
            page["transform_page_digest"] = json!(transform_page_content_digest(&page));
            page
        })
        .collect()
}

async fn send_pages(handler: &Handler, whole: Value) -> PreparedOutcome {
    let mut outcome = None;
    for page in pages(&with_base(whole)) {
        outcome = Some(raw(handler, page).await);
    }
    outcome.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request() {
    let (handler, store, _dir) = folded().await;
    let before = durable(&store);
    let window = body(&["m4", "m5", "m6"], anchor("m4", 2));

    let mut revision_2 = window.clone();
    revision_2["v"] = json!(2);
    let (code, _) = error_frame(send_pages(&handler, revision_2).await);
    assert_eq!(code, "transform_revision_unsupported");
    let mut missing = window.clone();
    missing.as_object_mut().unwrap().remove("boundary");
    let (code, _) = error_frame(send_pages(&handler, missing).await);
    assert_eq!(code, "invalid_params");
    let mut off_head = window.clone();
    off_head["boundary"] = anchor("m5", 2);
    let (code, _) = error_frame(send_pages(&handler, off_head).await);
    assert_eq!(code, "invalid_params");

    // `v` on a non-final page is refused by the paging rules before any assembly.
    let mut early = pages(&with_base(window.clone()));
    early[0]["v"] = json!(3);
    early[0]["transform_page_digest"] = json!(transform_page_content_digest(&early[0]));
    let (code, _) = error_frame(raw(&handler, early.remove(0)).await);
    assert_eq!(code, "authority_transform_page_protocol_mismatch");
    let (row_version, meta, _) = durable(&store);
    assert_eq!((row_version, meta), (before.0, before.1));

    let PreparedOutcome::Response(bytes) = send_pages(&handler, window).await else {
        panic!("the assembled revision 3 request is served");
    };
    let served: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(served["status"], "ok", "{served}");
    assert_eq!(served["boundary"], anchor("m4", 2));
}

#[tokio::test(flavor = "current_thread")]
async fn each_resolution_outcome_runs_through_the_handler_with_its_effects() {
    let (handler, store, _dir) = handler_for("rev3");
    store
        .replace_history_segments(session(), &[segment(1, 1, 2), segment(2, 3, 4)])
        .unwrap();

    // FirstPass: no coverage, the whole array numbered from 1.
    let first = body(&["m1", "m2", "m3", "m4", "m5", "m6"], Value::Null);
    let resolved = resolution(&store, &first);
    assert_eq!(resolved.resolution, Resolution::FirstPass);
    assert_eq!(resolved.ordinals, vec![1, 2, 3, 4, 5, 6]);
    let response = call(&handler, first).await;
    assert_eq!(response["action"], "HARD");
    assert_eq!(response["boundary"], anchor("m4", 2));
    assert_eq!(served_mids(&response), ["m5", "m6"]);
    assert!(response.get("coverage_ordinal").is_none());
    assert!(response.get("ordinal_continuation_base").is_none());
    for field in ["messages", "operations"] {
        assert!(
            !response[field].to_string().contains("\"ordinal\""),
            "{field}"
        );
    }
    assert_eq!(
        store.load(session()).unwrap().meta.coverage_ordinal,
        Some(4)
    );

    // Normal: the declared row is the rendered boundary; ordinals continue from its end.
    let normal = body(&["m4", "m5", "m6", "m7"], anchor("m4", 2));
    let resolved = resolution(&store, &normal);
    assert_eq!(resolved.resolution, Resolution::Normal);
    assert_eq!(resolved.ordinals, vec![4, 5, 6, 7]);
    let response = call(&handler, normal).await;
    assert_eq!(response["boundary"], anchor("m4", 2));
    assert_eq!(served_mids(&response), ["m5", "m6", "m7"]);

    // StaleSlice: a fold advanced coverage past the declared row, whose mid is still the head.
    store
        .append_history_segments(session(), &[segment(3, 5, 6)])
        .unwrap();
    store.arm_soft_refresh(session()).unwrap();
    let folded = call(
        &handler,
        body(&["m4", "m5", "m6", "m7", "m8"], anchor("m4", 2)),
    )
    .await;
    assert_eq!(folded["action"], "SOFT");
    assert_eq!(folded["boundary"], anchor("m6", 3));
    let stale = body(&["m4", "m5", "m6", "m7", "m8"], anchor("m4", 2));
    let resolved = resolution(&store, &stale);
    assert_eq!(resolved.resolution, Resolution::StaleSlice { cut: 2 });
    assert_eq!(resolved.anchor.as_ref().unwrap().sequence, 3);
    assert_eq!(resolved.ordinals, vec![6, 7, 8]);
    let sliced = call(&handler, stale).await;
    let observed = ["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8"];
    assert_eq!(
        identity_mids(&store),
        observed,
        "omitted mids keep their rows"
    );
    let declared = call(&handler, body(&["m6", "m7", "m8"], anchor("m6", 3))).await;
    assert_eq!(served_mids(&sliced), ["m7", "m8"]);
    assert_eq!(sliced["messages"], declared["messages"]);
    assert_eq!(sliced["boundary"], anchor("m6", 3));

    // Revert through a surviving anchor: SOFT with reconcile pending, then HARD truncates.
    let reverted = body(&["m2", "n3"], anchor("m2", 1));
    let resolved = resolution(&store, &reverted);
    assert_eq!(
        resolved.resolution,
        Resolution::Revert {
            keep_through_seq: Some(1)
        }
    );
    assert_eq!(resolved.ordinals, vec![2, 3]);
    let epoch = store.load(session()).unwrap().meta.revert_epoch;
    let soft = call(&handler, reverted.clone()).await;
    assert_eq!(soft["action"], "SOFT+");
    assert_eq!(soft["reconcile_pending"], true);
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 3);
    let with_revert: Vec<&str> = observed.iter().copied().chain(["n3"]).collect();
    assert_eq!(identity_mids(&store), with_revert);
    let hard = call(&handler, reverted).await;
    assert_eq!(hard["action"], "HARD");
    assert_eq!(identity_mids(&store), with_revert);
    assert_eq!(hard["reconcile_pending"], false);
    assert_eq!(hard["boundary"], anchor("m2", 1));
    assert_eq!(served_mids(&hard), ["n3"]);
    let loaded = store.load(session()).unwrap();
    assert_eq!(loaded.meta.revert_epoch, epoch + 1);
    assert_eq!(loaded.meta.coverage_ordinal, Some(2));
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 1);

    // Unknown: the declared sequence names no row; no recipe and nothing written.
    let before = durable(&store);
    let unknown = call(&handler, body(&["m2", "n3"], anchor("m2", 7))).await;
    assert_eq!(unknown["status"], "boundary_unknown");
    assert_eq!(unknown["action"], "BOUNDARY_UNKNOWN");
    assert!(unknown.get("operations").is_none());
    assert!(unknown.get("boundary").is_none());
    assert_eq!(durable(&store), before);

    // Revert through no anchor: the session is reset and the window served as a first pass.
    let none = body(&["x1"], Value::Null);
    let resolved = resolution(&store, &none);
    assert_eq!(
        resolved.resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
    assert_eq!(resolved.ordinals, vec![1]);
    let reset = call(&handler, none).await;
    assert_eq!(reset["action"], "HARD");
    assert_eq!(reset["boundary"], Value::Null);
    assert_eq!(served_mids(&reset), ["x1"]);
    let loaded = store.load(session()).unwrap();
    assert!(loaded.meta.pending_rewrite.is_none());
    assert_eq!(loaded.meta.revert_epoch, epoch + 2);
    assert!(store.load_history_segments(session()).unwrap().is_empty());
}

/// Acceptance (spec D10, WP-E03 disposition): a revert before the first anchor resets the
/// session in the same pass and serves the window as a first pass numbered from 1; the next
/// fold starts from the surviving array. The pass-through arm is not taken without
/// `lineage_switched`.
#[tokio::test(flavor = "current_thread")]
async fn a_revert_before_the_first_anchor_resets_and_serves_the_window_as_a_first_pass() {
    let (handler, store, _dir) = folded().await;
    let epoch = store.load(session()).unwrap().meta.revert_epoch;
    assert_eq!(
        crate::transform::removed_sequence_range(store.history_segment_ends(session()).unwrap()),
        "1..=2"
    );
    assert_eq!(crate::transform::removed_sequence_range(None), "none");
    let window = body(&["x1", "x2"], Value::Null);
    assert_eq!(
        resolution(&store, &window).resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
    let served = call(&handler, window).await;
    assert_eq!(served["status"], "ok", "{served}");
    assert_eq!(served["action"], "HARD");
    assert_eq!(served["boundary"], Value::Null);
    assert_eq!(served_mids(&served), ["x1", "x2"]);
    let loaded = store.load(session()).unwrap();
    assert_eq!(loaded.meta.revert_epoch, epoch + 1);
    assert!(loaded.meta.pending_rewrite.is_none());
    assert_eq!(loaded.core.boundary_id, "");
    assert!(store.load_history_segments(session()).unwrap().is_empty());

    // A fold over the surviving array anchors the next pass at its own ordinals.
    store
        .append_history_segments(
            session(),
            &[StoredHistorySegment {
                start_message_id: "x1#0".to_string(),
                end_message_id: "x2#0".to_string(),
                ..segment(1, 1, 2)
            }],
        )
        .unwrap();
    store.arm_soft_refresh(session()).unwrap();
    let next = body(&["x1", "x2", "x3"], Value::Null);
    let resolved = resolution(&store, &next);
    assert_eq!(resolved.resolution, Resolution::FirstPass);
    assert_eq!(resolved.ordinals, vec![1, 2, 3]);
    let folded = call(&handler, next).await;
    assert_eq!(folded["status"], "ok", "{folded}");
    assert_eq!(folded["boundary"], anchor("x2", 1));
    assert_eq!(served_mids(&folded), ["x3"]);
}

/// A write between the reset's resolution and its commit fails the reset's CAS; the pass
/// resolves again, resets once, and serves the first pass.
#[tokio::test(flavor = "current_thread")]
async fn a_cas_conflict_on_the_no_survivor_reset_resolves_again_and_resets_once() {
    let (handler, store, _dir) = folded_as("rev3-reset-now-cas").await;
    let epoch = store.load(session()).unwrap().meta.revert_epoch;
    let hook_store = Arc::clone(&store);
    let conflicted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&conflicted);
    install_transform_attempt_hook(session(), move || {
        let loaded = hook_store.load("rev3-reset-now-cas").unwrap();
        hook_store
            .commit(
                "rev3-reset-now-cas",
                loaded.row_version,
                &loaded.core,
                &loaded.meta,
            )
            .unwrap();
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let served = call(&handler, body(&["x1", "x2"], Value::Null)).await;
    assert!(conflicted.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(served["status"], "ok", "{served}");
    assert_eq!(served["boundary"], Value::Null);
    assert_eq!(served_mids(&served), ["x1", "x2"]);
    assert_eq!(store.load(session()).unwrap().meta.revert_epoch, epoch + 1);
    assert!(store.load_history_segments(session()).unwrap().is_empty());
}

/// A revert under pressure with a pending todo injection, over the folded session plus segment
/// 3, whose end is `end`. The revert's first pass defers with the reconcile pending and prunes
/// no identity (spec 7.10.4); returns the second pass's answer.
async fn pressured_revert(handler: &Handler, store: &MemoryStore, end: &str) -> Value {
    store
        .append_history_segments(
            session(),
            &[StoredHistorySegment {
                end_message_id: end.to_string(),
                ..segment(3, 5, 6)
            }],
        )
        .unwrap();
    store.arm_soft_refresh(session()).unwrap();
    let loaded = store.load(session()).unwrap();
    let mut meta = loaded.meta.clone();
    meta.last_todo_state =
        Some(r#"[{"content":"t","status":"in_progress","priority":"high"}]"#.to_string());
    store
        .commit(session(), loaded.row_version, &loaded.core, &meta)
        .unwrap();
    let mut reverted = body(&["m2", "n3"], anchor("m2", 1));
    reverted["history_budget_tokens"] = json!(1.0);
    reverted["todo_tool_present"] = json!(true);
    assert_eq!(
        resolution(store, &reverted).resolution,
        Resolution::Revert {
            keep_through_seq: Some(1)
        }
    );
    let identities = identity_mids(store);
    let deferred = call(handler, reverted.clone()).await;
    assert_eq!(deferred["action"], "SOFT+", "{deferred}");
    assert_eq!(deferred["reconcile_pending"], true, "{deferred}");
    assert_eq!(served_mids(&deferred), ["n3"], "{deferred}");
    let kept = identity_mids(store);
    assert!(identities.iter().all(|mid| kept.contains(mid)), "{kept:?}");
    call(handler, reverted).await
}

/// The revert's HARD after its defer keeps every identity row it inherited, whether segment 3
/// ended at a mid live in the reverted window or at one the revert removed.
#[tokio::test(flavor = "current_thread")]
async fn a_revert_under_pressure_defers_then_folds_and_keeps_the_identity_history() {
    for end in ["n3#0", "m6#0"] {
        let (handler, store, _dir) = folded().await;
        let before = identity_mids(&store);
        let folded = pressured_revert(&handler, &store, end).await;
        assert_eq!(folded["status"], "ok", "{end}: {folded}");
        assert_eq!(folded["action"], "HARD", "{end}: {folded}");
        assert_eq!(folded["reconcile_pending"], false, "{end}: {folded}");
        let after = identity_mids(&store);
        assert!(
            before
                .iter()
                .chain(&["m2".to_string(), "n3".to_string()])
                .all(|mid| after.contains(mid)),
            "{end}: {after:?}"
        );
    }
}

/// The pass that starts a revert's reconcile serves every message after the declared anchor,
/// though their ordinals reuse numbers the reverted coverage still counts: here n3 to n5 take
/// ordinals 3 to 5 under a coverage ordinal of 4.
#[tokio::test(flavor = "current_thread")]
async fn the_reconciling_pass_of_a_revert_serves_the_messages_after_the_declared_anchor() {
    let (handler, store, _dir) = folded().await;
    let reverted = body(&["m2", "n3", "n4", "n5"], anchor("m2", 1));
    assert_eq!(
        resolution(&store, &reverted).resolution,
        Resolution::Revert {
            keep_through_seq: Some(1)
        }
    );
    let reconciling = call(&handler, reverted).await;
    assert_eq!(reconciling["status"], "ok", "{reconciling}");
    assert_eq!(reconciling["action"], "SOFT+", "{reconciling}");
    assert_eq!(reconciling["reconcile_pending"], true, "{reconciling}");
    assert_eq!(
        served_mids(&reconciling),
        ["n3", "n4", "n5"],
        "{reconciling}"
    );
}

/// Arms `id`'s attempt hook to run `step` on each firing with its 1-based count, re-arming
/// while `step` returns true. The hook runs on the pass's thread, where `session()` is not the
/// test's session. Returns the firing count.
fn arm_steps(
    id: &'static str,
    step: impl Fn(u32) -> bool + Send + Sync + 'static,
) -> Arc<std::sync::atomic::AtomicU32> {
    type Step = Arc<dyn Fn(u32) -> bool + Send + Sync>;
    fn arm(id: &'static str, fired: Arc<std::sync::atomic::AtomicU32>, step: Step) {
        install_transform_attempt_hook(id, move || {
            let count = fired.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if step(count) {
                arm(id, Arc::clone(&fired), Arc::clone(&step));
            }
        });
    }
    let fired = Arc::new(std::sync::atomic::AtomicU32::new(0));
    arm(id, Arc::clone(&fired), Arc::new(step));
    fired
}

/// A commit that moves `id`'s row version under the pass.
fn bump(store: &MemoryStore, id: &str) {
    let loaded = store.load(id).unwrap();
    store
        .commit(id, loaded.row_version, &loaded.core, &loaded.meta)
        .unwrap();
}

/// A reset that loses its CAS on every attempt surfaces the conflict once the retry budget is
/// spent.
#[tokio::test(flavor = "current_thread")]
async fn a_no_survivor_reset_that_always_loses_its_cas_surfaces_the_conflict() {
    let (handler, store, _dir) = folded_as("rev3-reset-cas-exhausted").await;
    let hook_store = Arc::clone(&store);
    let fired = arm_steps("rev3-reset-cas-exhausted", move |_| {
        bump(&hook_store, "rev3-reset-cas-exhausted");
        true
    });
    let served = call(&handler, body(&["x1", "x2"], Value::Null)).await;
    install_transform_attempt_hook(session(), || {});
    assert_eq!(served["status"], "error", "{served}");
    assert_eq!(served["code"], "transform_failed", "{served}");
    assert!(
        served["message"].as_str().unwrap().contains("cas conflict"),
        "{served}"
    );
    assert_eq!(
        fired.load(std::sync::atomic::Ordering::SeqCst),
        crate::transform::MAX_CAS_RETRIES + 1
    );
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 2);
}

/// A committed reset spends no retry: the full budget of transform conflicts after it still
/// ends in a committed first pass.
#[tokio::test(flavor = "current_thread")]
async fn a_committed_reset_leaves_the_whole_retry_budget_to_the_pass() {
    let (handler, store, _dir) = folded_as("rev3-reset-then-conflicts").await;
    let hook_store = Arc::clone(&store);
    let retries = crate::transform::MAX_CAS_RETRIES;
    // Firing 1 is the reset; firings 2 through `retries + 1` are the pass's transform commits.
    let fired = arm_steps("rev3-reset-then-conflicts", move |count| {
        if count > 1 {
            bump(&hook_store, "rev3-reset-then-conflicts");
        }
        count <= retries
    });
    let served = call(&handler, body(&["x1", "x2"], Value::Null)).await;
    assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), retries + 1);
    assert_eq!(served["status"], "ok", "{served}");
    assert_eq!(served["boundary"], Value::Null);
    assert_eq!(served_mids(&served), ["x1", "x2"]);
    assert!(store.load_history_segments(session()).unwrap().is_empty());
}

/// One reset per pass: a writer that restores the pre-reset coverage before the pass commits
/// brings a second no-survivor resolution, which fails the pass instead of resetting again.
#[tokio::test(flavor = "current_thread")]
async fn a_second_no_survivor_resolution_in_one_pass_fails_with_a_cas_conflict() {
    let (handler, store, _dir) = folded_as("rev3-reset-twice").await;
    let before = store.load("rev3-reset-twice").unwrap();
    let segments = store.load_history_segments("rev3-reset-twice").unwrap();
    let hook_store = Arc::clone(&store);
    // Firing 1 is the reset; firing 2, at the transform commit, restores the folded session.
    let fired = arm_steps("rev3-reset-twice", move |count| {
        if count == 2 {
            hook_store
                .replace_history_segments("rev3-reset-twice", &segments)
                .unwrap();
            let loaded = hook_store.load("rev3-reset-twice").unwrap();
            hook_store
                .commit(
                    "rev3-reset-twice",
                    loaded.row_version,
                    &before.core,
                    &before.meta,
                )
                .unwrap();
        }
        count < 2
    });
    let window = body(&["x1", "x2"], Value::Null);
    let served = call(&handler, window.clone()).await;
    assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(served["status"], "error", "{served}");
    assert_eq!(served["code"], "transform_failed", "{served}");
    assert!(
        served["message"].as_str().unwrap().contains("cas conflict"),
        "{served}"
    );
    assert_eq!(
        resolution(&store, &window).resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 2);
}

/// A same-pass reset bumps the revert epoch, so neither the CK nor the native previous output
/// of the reset session is a keep source, though the served window repeats its bytes.
#[tokio::test(flavor = "current_thread")]
async fn a_same_pass_reset_reuses_no_previous_output() {
    let (handler, store, _dir) = folded().await;
    let repeat = call(&handler, body(&["m4", "m5", "m6"], anchor("m4", 2))).await;
    assert!(!previous_keeps(&repeat).is_empty(), "{repeat}");
    let window = body(&["m5", "m6"], Value::Null);
    assert_eq!(
        resolution(&store, &window).resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
    let reset = call(&handler, window).await;
    assert_eq!(served_mids(&reset), ["m5", "m6"]);
    assert_eq!(previous_keeps(&reset), Vec::<Value>::new(), "{reset}");

    let (handler, store, _dir) = handler_for("rev3-native-reset");
    store
        .replace_history_segments(session(), &[segment(1, 1, 2), segment(2, 3, 4)])
        .unwrap();
    let names = mids(1..=6);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    call(&handler, native_body(&names, Value::Null)).await;
    let repeat = call(&handler, native_body(&["m4", "m5", "m6"], anchor("m4", 2))).await;
    assert!(!previous_keeps(&repeat).is_empty(), "{repeat}");
    let window = native_body(&["m5", "m6"], Value::Null);
    assert_eq!(
        resolution(&store, &window).resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
    let reset = call(&handler, window).await;
    assert_eq!(reset["status"], "ok", "{reset}");
    assert_eq!(previous_keeps(&reset), Vec::<Value>::new(), "{reset}");
}

/// A revert through no anchor that the pending-rewrite arm does not take: a lineage-switched
/// window over a session that carries a lineage anchor and continuation base, as lineage
/// descent writes them, so the reset of spec D10 does not run. Its first pass defers; its
/// second removes every segment, and `interrupt` runs between that removal's commit and the
/// pass's terminal commit. Returns the window and the interrupted pass's own answer.
async fn unanchored_revert(
    id: &'static str,
    interrupt: impl Fn(&MemoryStore, &str) + Send + Sync + 'static,
) -> (
    Handler,
    Arc<MemoryStore>,
    tempfile::TempDir,
    u64,
    Value,
    Value,
) {
    let (handler, store, dir) = handler_for(id);
    store
        .replace_history_segments(session(), &[segment(1, 1, 2), segment(2, 3, 4)])
        .unwrap();
    let names = mids(1..=6);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(
        call(&handler, body(&names, Value::Null)).await["action"],
        "HARD"
    );
    let loaded = store.load(session()).unwrap();
    let mut meta = loaded.meta.clone();
    meta.anchor_block_id = Some("m1#0".to_string());
    // The window never carries `m1`, so the hash is never compared.
    meta.anchor_content_hash = Some("0".repeat(64));
    meta.ordinal_continuation_base = Some(6);
    meta.descent_completed = true;
    store
        .commit(session(), loaded.row_version, &loaded.core, &meta)
        .unwrap();
    let mut window = body(&["x1", "x2"], Value::Null);
    for (field, value) in [
        ("lineage_switched", json!(true)),
        ("descent_edge_id", json!(1)),
        ("prior_conversation_key", json!("prior")),
        ("prior_epoch", json!(1)),
        ("new_epoch", json!(2)),
        ("constituents", json!([["prior", id, 2]])),
    ] {
        window[field] = value;
    }
    assert_eq!(
        resolution(&store, &window).resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
    let soft = call(&handler, window.clone()).await;
    assert_eq!(soft["reconcile_pending"], true, "{soft}");
    let hook_store = Arc::clone(&store);
    install_transform_attempt_hook(id, move || {
        assert!(
            hook_store.load_history_segments(id).unwrap().is_empty(),
            "the interruption lands after the removal commit"
        );
        interrupt(&hook_store, id);
    });
    let answer = call(&handler, window.clone()).await;
    (handler, store, dir, meta.revert_epoch, window, answer)
}

/// Removing every segment is the reset of spec D10: the continuation base goes with the
/// history, so the next `null` pass is a first pass numbered from 1, not a lineage refusal.
async fn assert_reset_converges(handler: &Handler, store: &MemoryStore, epoch: u64, window: Value) {
    assert!(store.load_history_segments(session()).unwrap().is_empty());
    let loaded = store.load(session()).unwrap();
    assert_eq!(loaded.meta.revert_epoch, epoch + 1);
    assert_eq!(loaded.meta.ordinal_continuation_base, None);
    assert_eq!(loaded.meta.anchor_block_id, None);
    assert_eq!(loaded.meta.anchor_content_hash, None);
    assert!(!loaded.meta.descent_completed);
    let resolved = resolution(store, &window);
    assert_eq!(resolved.resolution, Resolution::FirstPass);
    assert_eq!(resolved.ordinals, vec![1, 2]);
    let next = call(handler, window).await;
    assert_eq!(next["status"], "ok", "{next}");
    assert_eq!(next["boundary"], Value::Null);
    assert_eq!(served_mids(&next), ["x1", "x2"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_revert_through_no_anchor_outside_the_pending_rewrite_arm_removes_every_segment() {
    let (handler, store, _dir, epoch, window, hard) =
        unanchored_revert("rev3-reset", |_, _| {}).await;
    assert_eq!(hard["action"], "HARD", "{hard}");
    assert_eq!(hard["reconcile_pending"], false);
    assert_eq!(hard["boundary"], Value::Null);
    // `Revert { None }` stamps the window without the continuation base the reset clears, so
    // what the pass served and hands the summarizer is numbered as the next first pass numbers it.
    let TransformSnapshotLookup::Ready(lease) =
        handler.transform_snapshots.lock().unwrap().get(session())
    else {
        panic!("the reset pass publishes a ready snapshot");
    };
    let ordinals: Vec<u64> = lease.request.messages.iter().map(|m| m.ordinal).collect();
    assert_eq!(ordinals, [1, 2]);
    drop(lease);
    assert_reset_converges(&handler, &store, epoch, window).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_cas_conflict_after_the_unanchored_revert_removal_converges() {
    let (handler, store, _dir, epoch, window, answer) =
        unanchored_revert("rev3-reset-cas", |store, session| {
            let loaded = store.load(session).unwrap();
            store
                .commit(session, loaded.row_version, &loaded.core, &loaded.meta)
                .unwrap();
        })
        .await;
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(answer["boundary"], Value::Null);
    assert_reset_converges(&handler, &store, epoch, window).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_panic_plus_reopen_after_the_unanchored_revert_removal_converges() {
    let (handler, store, dir, epoch, window, answer) =
        unanchored_revert("rev3-reset-kill", |_, _| {
            panic!("the pass dies after the removal commit")
        })
        .await;
    assert_eq!(answer["code"], "internal_error", "{answer}");
    drop(handler);
    drop(store);
    let (handler, store) = reopened(&dir);
    assert_reset_converges(&handler, &store, epoch, window).await;
}

/// A publish during a null-boundary pass cannot move its cut: only rows the rendered boundary
/// covers are stale-slice candidates, so the newer row is folded rather than cut at, and m3 is
/// not dropped. The ready snapshot holds the resolved window the pass folded (its input).
#[tokio::test(flavor = "current_thread")]
async fn a_publish_during_a_null_boundary_pass_keeps_the_cut_at_the_rendered_boundary() {
    let (handler, store, _dir) = handler_for("rev3-cut");
    store
        .replace_history_segments(session(), &[segment(1, 1, 2)])
        .unwrap();
    let host = mids(1..=7);
    let host: Vec<&str> = host.iter().map(String::as_str).collect();
    let first = call(&handler, body(&host[..6], Value::Null)).await;
    assert_eq!(first["boundary"], anchor("m2", 1));
    let window = body(&host, Value::Null);
    assert_eq!(
        resolution(&store, &window).resolution,
        Resolution::StaleSlice { cut: 1 }
    );
    let hook_store = Arc::clone(&store);
    let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&published);
    install_transform_attempt_hook(session(), move || {
        hook_store
            .append_history_segments("rev3-cut", &[segment(2, 3, 4)])
            .unwrap();
        hook_store.arm_soft_refresh("rev3-cut").unwrap();
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let served = call(&handler, window).await;
    assert_eq!(served["status"], "ok", "{served}");
    assert!(published.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(served_mids(&served), ["m5", "m6", "m7"]);
    assert_eq!(served["boundary"], anchor("m4", 2));
    // The fold rendered segment 2 (m3..m4), so m3 is covered rather than dropped.
    let core = store.load("rev3-cut").unwrap().core;
    assert!(core.boundary_id.starts_with("m4#"), "{}", core.boundary_id);
    assert_eq!(
        ready_snapshot_mids(&handler),
        ["m2", "m3", "m4", "m5", "m6", "m7"]
    );
}

/// A recomp reset after the handler's resolution moves the cut (the retry resolves a first
/// pass); the retry serves the new cut, and the ready snapshot holds that window, not the first.
#[tokio::test(flavor = "current_thread")]
async fn a_reset_that_moves_the_cut_leaves_the_served_window_in_the_ready_snapshot() {
    let (handler, store, _dir) = handler_for("rev3-reset-cut");
    store
        .replace_history_segments(session(), &[segment(1, 1, 2)])
        .unwrap();
    let host = mids(1..=7);
    let host: Vec<&str> = host.iter().map(String::as_str).collect();
    let first = call(&handler, body(&host[..6], Value::Null)).await;
    assert_eq!(first["boundary"], anchor("m2", 1));
    let hook_store = Arc::clone(&store);
    install_transform_attempt_hook(session(), move || {
        let loaded = hook_store.load("rev3-reset-cut").unwrap();
        hook_store
            .reset_session_for_recomp("rev3-reset-cut", loaded.row_version)
            .unwrap();
    });
    let served = call(&handler, body(&host, Value::Null)).await;
    assert_eq!(served["status"], "ok", "{served}");
    let whole = ["m1", "m2", "m3", "m4", "m5", "m6", "m7"];
    assert_eq!(served_mids(&served), whole);
    assert_eq!(served["boundary"], Value::Null);
    assert_eq!(ready_snapshot_mids(&handler), whole);
}

fn ready_snapshot_mids(handler: &Handler) -> Vec<String> {
    let TransformSnapshotLookup::Ready(lease) =
        handler.transform_snapshots.lock().unwrap().get(session())
    else {
        panic!("the accepted pass publishes a ready snapshot");
    };
    lease
        .request
        .messages
        .iter()
        .map(|message| message.mid.clone())
        .collect()
}

fn native_user(mid: &str) -> Value {
    json!({
        "info": { "id": mid, "role": "user" },
        "parts": [{ "type": "text", "text": format!("text of {mid}") }],
    })
}

/// The native form of `body`: the CK messages decoded from the native window.
fn native_body(mids: &[&str], boundary: Value) -> Value {
    let native: Vec<Value> = mids.iter().map(|mid| native_user(mid)).collect();
    let decoded = crate::codec::decode_opencode(&native);
    let messages: Vec<Value> = decoded
        .messages
        .iter()
        .map(|message| json!({ "mid": message.mid, "ck": message.ck }))
        .collect();
    json!({
        "kind": "transform",
        "v": 3,
        "boundary": boundary,
        "serializer_profile": "opencode-aisdk",
        "provider_id": "anthropic",
        "serve_native": true,
        "session_id": session(),
        "render_config": "cfg0",
        "messages": messages,
        "native_messages": native,
    })
}

/// WP-P05 at the handler: after a stale cut the recipe's input keeps address the submitted
/// window, and the test client's application against that window reconstructs the output.
#[tokio::test(flavor = "current_thread")]
async fn stale_slice_input_keeps_address_the_submitted_native_window() {
    let (handler, store, _dir) = handler_for("rev3");
    store
        .replace_history_segments(session(), &[segment(1, 1, 2)])
        .unwrap();
    let first = call(&handler, native_body(&["m1", "m2", "m3"], Value::Null)).await;
    assert_eq!(first["boundary"], anchor("m2", 1));
    store
        .append_history_segments(session(), &[segment(2, 3, 4)])
        .unwrap();
    store.arm_soft_refresh(session()).unwrap();
    let window = ["m2", "m3", "m4", "m5", "m6"];
    let folded = call(&handler, native_body(&window, anchor("m2", 1))).await;
    assert_eq!(folded["boundary"], anchor("m4", 2));
    let stale = native_body(&window, anchor("m2", 1));
    assert_eq!(
        resolution(&store, &stale).resolution,
        Resolution::StaleSlice { cut: 2 }
    );
    // No previous output is offered, so unchanged messages can only be kept from the input.
    let mut stale = stale;
    stale["previous_output_revision"] = Value::Null;
    let response = call(&handler, stale).await;
    let keeps: Vec<u64> = response["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["op"] == "keep" && op["source"] == "input")
        .map(|op| op["start"].as_u64().unwrap())
        .collect();
    assert!(!keeps.is_empty(), "{response}");
    assert!(keeps.iter().all(|start| *start >= 2), "{keeps:?}");
    let served: Vec<&str> = response["native_messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["info"]["id"].as_str())
        .filter(|id| id.starts_with('m'))
        .collect();
    assert_eq!(served, ["m5", "m6"]);
}

/// A handler over the store at `dir`, bound as `golden_handler` binds it.
fn reopened(dir: &tempfile::TempDir) -> (Handler, Arc<MemoryStore>) {
    let data_home = dir.path().join("data");
    let store =
        Arc::new(MemoryStore::open(&dev_descriptor_at(data_home.to_str().unwrap())).unwrap());
    let handler = Handler::with_producer_factory_config_resolver(
        Arc::new(TestProducerFactory {
            state: Arc::new(ProducerState::default()),
        }),
        default_test_config(),
        Arc::new(MissingSessionResolver),
    );
    handler.install_store_for_test(Arc::clone(&store));
    let mut binding = binding(dir.path().join("project").to_str().unwrap(), session());
    binding.config.temporal_awareness = false;
    binding.config.inject_docs = false;
    handler.bind_route(test_route(7), binding);
    (handler, store)
}

/// One discovery walk and transform as the plugin runs them for `host`: the newest listed
/// anchor present in the host, else `null` with the whole host.
async fn plugin_pass(handler: &Handler, host: &[&str]) -> Value {
    let mut before = None::<i64>;
    loop {
        let mut request =
            json!({ "method": "transform.boundary", "v": 3, "session_id": session() });
        if let Some(before) = before {
            request["before_sequence"] = json!(before);
        }
        let PreparedOutcome::Response(bytes) = handler.dispatch_value(test_route(7), request).await
        else {
            panic!("discovery answers a page");
        };
        let page: Value = serde_json::from_slice(&bytes).unwrap();
        let anchors = page["anchors"].as_array().unwrap();
        let Some(last) = anchors.last() else {
            return call(handler, body(host, Value::Null)).await;
        };
        if let Some((at, found)) = anchors.iter().find_map(|found| {
            let at = host.iter().position(|mid| found["mid"] == *mid)?;
            Some((at, found))
        }) {
            return call(handler, body(&host[at..], found.clone())).await;
        }
        before = last["sequence"].as_i64();
    }
}

/// The interrupted-revert history of spec D10. The revert's first pass defers with reconcile
/// pending; its second truncates to the surviving anchor, and `interrupt` runs between that
/// commit and the pass's terminal commit. Returns the interrupted pass's own answer.
async fn interrupted_revert(
    id: &'static str,
    interrupt: impl Fn(&MemoryStore, &str) + Send + Sync + 'static,
) -> (Handler, Arc<MemoryStore>, tempfile::TempDir, u64, Value) {
    let (handler, store, dir) = handler_for(id);
    store
        .replace_history_segments(session(), &[segment(1, 1, 2), segment(2, 3, 4)])
        .unwrap();
    let names = mids(1..=6);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(
        plugin_pass(&handler, &names).await["boundary"],
        anchor("m4", 2)
    );
    let epoch = store.load(session()).unwrap().meta.revert_epoch;
    // The host reverts past m4 and appends n3; discovery finds m2.
    assert_eq!(plugin_pass(&handler, &REVERTED).await["action"], "SOFT+");
    let hook_store = Arc::clone(&store);
    install_transform_attempt_hook(id, move || {
        assert_eq!(
            hook_store.load_history_segments(id).unwrap().len(),
            1,
            "the interruption lands after the truncate commit"
        );
        interrupt(&hook_store, id);
    });
    let answer = call(&handler, retained_pass()).await;
    (handler, store, dir, epoch, answer)
}

/// The host after the revert, and the pass the plugin sends with its retained anchor (m2, 1).
const REVERTED: [&str; 3] = ["m1", "m2", "n3"];

fn retained_pass() -> Value {
    body(&REVERTED[1..], anchor("m2", 1))
}

/// The fold D10 names: a HARD against the surviving anchor, the epoch moved once, one segment.
fn assert_folded(store: &MemoryStore, answer: &Value, epoch: u64) {
    assert_eq!(answer["action"], "HARD", "{answer}");
    assert_eq!(answer["reconcile_pending"], false);
    assert_eq!(answer["boundary"], anchor("m2", 1));
    assert_eq!(served_mids(answer), ["n3"]);
    assert_eq!(store.load(session()).unwrap().meta.revert_epoch, epoch + 1);
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 1);
}

/// After the fold: its compose minted the surviving anchor live, so it skipped the truncate
/// branch (D10's no-op path); a truncate re-entered at the same keep point changes neither
/// epoch nor row version, and the next pass is steady.
async fn assert_steady(handler: &Handler, store: &MemoryStore, epoch: u64) {
    let loaded = store.load(session()).unwrap();
    let again = store
        .truncate_history_segments_for_revert(session(), 1, loaded.row_version)
        .unwrap();
    assert_eq!(again.revert_epoch, epoch + 1);
    assert_eq!(Some(again.row_version), loaded.row_version);
    let steady = plugin_pass(handler, &REVERTED).await;
    assert_eq!(steady["action"], "SOFT+");
    assert_eq!(store.load(session()).unwrap().meta.revert_epoch, epoch + 1);
}

/// The conflict makes the same request resolve again: the surviving anchor is now rendered, so
/// the retry folds against it and the request answers the HARD, never `boundary_unknown`.
#[tokio::test(flavor = "current_thread")]
async fn a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request() {
    let (handler, store, _dir, epoch, answer) = interrupted_revert("rev3-cas", |store, session| {
        let loaded = store.load(session).unwrap();
        store
            .commit(session, loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
    })
    .await;
    assert_folded(&store, &answer, epoch);
    assert_steady(&handler, &store, epoch).await;
}

/// A panic after the truncate commit, then a reopened store: discovery lists the surviving
/// anchor, and the plugin's retained anchor folds against it on the next pass with no
/// `boundary_unknown` and no `null` whole-array pass.
#[tokio::test(flavor = "current_thread")]
async fn a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass() {
    let (handler, store, dir, epoch, answer) = interrupted_revert("rev3-kill", |_, _| {
        panic!("the pass dies after the truncate commit")
    })
    .await;
    assert_eq!(answer["code"], "internal_error", "{answer}");
    drop(handler);
    drop(store);
    let (handler, store) = reopened(&dir);
    let truncated = store.load(session()).unwrap();
    assert_eq!(truncated.meta.revert_epoch, epoch + 1);
    assert!(truncated.core.reconcile_pending);
    let PreparedOutcome::Response(bytes) = handler
        .dispatch_value(
            test_route(7),
            json!({ "method": "transform.boundary", "v": 3, "session_id": session() }),
        )
        .await
    else {
        panic!("discovery answers a page");
    };
    let page: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page["anchors"], json!([anchor("m2", 1)]));
    let answer = call(&handler, retained_pass()).await;
    assert_folded(&store, &answer, epoch);
    assert_steady(&handler, &store, epoch).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_revert_on_a_render_config_change_defers_then_folds() {
    let (handler, store, _dir) = folded().await;
    let mut reverted = body(&["m2", "n3"], anchor("m2", 1));
    reverted["render_config"] = json!("cfg1");
    let deferred = call(&handler, reverted.clone()).await;
    assert_eq!(deferred["action"], "SOFT+", "{deferred}");
    assert_eq!(deferred["reconcile_pending"], true);
    assert_eq!(served_mids(&deferred), ["n3"], "{deferred}");
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 2);
    let hard = call(&handler, reverted).await;
    assert_eq!(hard["action"], "HARD", "{hard}");
    assert_eq!(hard["reconcile_pending"], false);
    assert_eq!(hard["boundary"], anchor("m2", 1));
    assert_eq!(served_mids(&hard), ["n3"]);
    assert_eq!(store.load_history_segments(session()).unwrap().len(), 1);
}
