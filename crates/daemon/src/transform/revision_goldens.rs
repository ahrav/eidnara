//! Revision 2 transform goldens: bounded generated traces of host-array edits and deterministic
//! summary publications, driven through the transform handler against a real SQLite store.
//! After every operation a step records the applied served array with every `ordinal` key
//! removed, the frozen m0 bytes, and the session's tag rows, which are the input tag protection
//! decides from.
//!
//! The recorder speaks revision 2, so it runs only on a daemon that serves revision 2. To
//! re-record, check out the commit that adds this file (it sits directly on the base) and run
//!
//! ```text
//! EIDNARA_RECORD_REV2_GOLDENS=1 cargo +1.98 test -p daemon --all-features --locked \
//!     revision_goldens::record_revision_2_goldens -- --ignored --exact
//! ```
//!
//! which rewrites `crates/daemon/tests/fixtures/transform-revision-2-goldens.json`.

use super::*;

pub(super) const GOLDENS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/transform-revision-2-goldens.json"
);

/// One host-array edit or publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Append,
    RevertTail,
    RevertPastBoundary,
    RevertBeforeFirstAnchor,
    EditLast,
    Publish,
}

impl Op {
    pub(super) fn name(self) -> &'static str {
        match self {
            Op::Append => "append",
            Op::RevertTail => "revert_tail",
            Op::RevertPastBoundary => "revert_past_boundary",
            Op::RevertBeforeFirstAnchor => "revert_before_first_anchor",
            Op::EditLast => "edit_last",
            Op::Publish => "publish",
        }
    }
}

/// The traces: one fixed trace that visits every operation, then seeded traces. Once a revert
/// removes every anchor, only appends and edits follow, since revision 2 served raw from then on.
pub(super) fn traces() -> Vec<(String, Vec<Op>)> {
    use Op::*;
    let mut traces = vec![(
        "fixed".to_string(),
        vec![
            Append,
            Append,
            Append,
            Publish,
            Append,
            RevertTail,
            Append,
            EditLast,
            Publish,
            Append,
            Append,
            Publish,
            Append,
            RevertPastBoundary,
            Append,
            Append,
            Publish,
            Append,
            RevertBeforeFirstAnchor,
            Append,
            EditLast,
        ],
    )];
    for seed in 1..=8u64 {
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        let mut ops = vec![Append, Append];
        let mut reset = false;
        while ops.len() < 16 {
            let choices: &[Op] = if reset {
                &[Append, EditLast]
            } else {
                &[
                    Append,
                    Append,
                    Publish,
                    Publish,
                    RevertTail,
                    RevertPastBoundary,
                    EditLast,
                    RevertBeforeFirstAnchor,
                ]
            };
            let op = choices[next() % choices.len()];
            reset |= op == RevertBeforeFirstAnchor;
            ops.push(op);
        }
        traces.push((format!("seed-{seed}"), ops));
    }
    traces
}

/// The host array: `(mid, role, text)`, in canonical order.
#[derive(Debug, Default)]
pub(super) struct Host {
    pub(super) messages: Vec<(String, &'static str, String)>,
    next_mid: usize,
    edits: usize,
}

impl Host {
    /// Applies `op`; publications write one segment through the store.
    pub(super) fn apply(&mut self, op: Op, store: &MemoryStore) {
        let segments = store.load_history_segments(SESSION).unwrap();
        let anchor_index = |end_id: &str| {
            self.messages
                .iter()
                .position(|(mid, _, _)| format!("{mid}#0") == end_id)
        };
        match op {
            Op::Append => {
                for role in ["user", "assistant"] {
                    self.next_mid += 1;
                    let mid = format!("m{:03}", self.next_mid);
                    let text = format!("{role} turn {mid}: {}", "context ".repeat(4));
                    self.messages.push((mid, role, text));
                }
            }
            Op::RevertTail => {
                let keep = self.messages.len().saturating_sub(2).max(1);
                self.messages.truncate(keep);
            }
            Op::RevertPastBoundary => {
                // Keep through the second-newest anchor still in the host, dropping the newest.
                let present: Vec<usize> = segments
                    .iter()
                    .filter_map(|segment| anchor_index(&segment.end_message_id))
                    .collect();
                if let [.., keep, _] = present[..] {
                    self.messages.truncate(keep + 1);
                }
            }
            Op::RevertBeforeFirstAnchor => {
                if let Some(first) = segments
                    .first()
                    .and_then(|segment| anchor_index(&segment.end_message_id))
                {
                    self.messages.truncate(first);
                }
            }
            Op::EditLast => {
                self.edits += 1;
                if let Some((_, _, text)) = self.messages.last_mut() {
                    text.push_str(&format!(" (edit {})", self.edits));
                }
            }
            Op::Publish => {
                // Cover the host from the newest segment's end through all but the last two
                // messages, when the newest segment's end is still in the host.
                let start = match segments.last() {
                    None => 0,
                    Some(last) => match anchor_index(&last.end_message_id) {
                        Some(index) => index + 1,
                        None => return,
                    },
                };
                let end = self.messages.len().saturating_sub(2);
                if end <= start {
                    return;
                }
                let (start_mid, _, _) = &self.messages[start];
                let (end_mid, _, _) = &self.messages[end - 1];
                let sequence = segments.last().map_or(1, |last| last.sequence + 1);
                store
                    .append_history_segments(
                        SESSION,
                        &[StoredHistorySegment {
                            sequence,
                            start_message: start as i64 + 1,
                            end_message: end as i64,
                            start_message_id: format!("{start_mid}#0"),
                            end_message_id: format!("{end_mid}#0"),
                            title: format!("Summary {sequence}"),
                            content: format!("summary of {start_mid}..{end_mid}"),
                            p1: Some(format!("summary of {start_mid}..{end_mid}")),
                            importance: 50,
                            ..Default::default()
                        }],
                    )
                    .unwrap();
                store.arm_soft_refresh(SESSION).unwrap();
            }
        }
    }

    /// The host messages as CK ingress values from `from`, with revision 2 ordinals when asked.
    pub(super) fn wire(&self, from: usize, ordinals: bool) -> Vec<Value> {
        self.messages[from..]
            .iter()
            .enumerate()
            .map(|(offset, (mid, role, text))| {
                let mut message = serde_json::to_value(super::wire_with_role(
                    mid,
                    (from + offset + 1) as u64,
                    role,
                    text,
                ))
                .unwrap();
                if !ordinals {
                    message.as_object_mut().unwrap().remove("ordinal");
                }
                message
            })
            .collect()
    }
}

pub(super) const SESSION: &str = "ses";

/// A handler over a fresh store, bound with the clock-dependent m0 inputs turned off.
pub(super) fn golden_handler() -> (Handler, Arc<MemoryStore>, tempfile::TempDir) {
    let (handler, store, dir, project) = super::handler_with_store(
        Arc::new(super::ProducerState::default()),
        super::default_test_config(),
    );
    let mut binding = super::binding(project.to_str().unwrap(), SESSION);
    binding.config.temporal_awareness = false;
    binding.config.inject_docs = false;
    handler.bind_route(test_route(7), binding);
    (handler, store, dir)
}

/// The transform body both revisions share; the caller adds `v`, `messages`, and `boundary`.
pub(super) fn pass_body(messages: Vec<Value>) -> Value {
    json!({
        "kind": "transform",
        "serializer_profile": "opencode-aisdk",
        "session_id": SESSION,
        "render_config": "cfg0",
        "tool_present": true,
        "usage": { "current_total_input_tokens": 1_000, "context_limit_tokens": 200_000 },
        "messages": messages,
    })
}

/// Removes every `ordinal` key, recursively.
pub(super) fn strip_ordinals(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("ordinal");
            object.values_mut().for_each(strip_ordinals);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_ordinals),
        _ => {}
    }
}

/// Runs one pass through the handler and the test client's recipe application; an error answer
/// reads as `{"status": "error", "code", "message"}`.
pub(super) async fn call(handler: &Handler, body: Value) -> Value {
    match handler
        .handle_transform_applied_for_test(test_route(7), body)
        .await
    {
        PreparedOutcome::Response(bytes) => serde_json::from_slice(&bytes).unwrap(),
        PreparedOutcome::Error { code, message } => {
            json!({ "status": "error", "code": code, "message": message })
        }
        other => panic!("unexpected handler outcome: {other:?}"),
    }
}

/// The recorded step after one operation.
pub(super) fn step(op: Op, response: &Value, store: &MemoryStore) -> Value {
    let mut served = response["messages"].clone();
    strip_ordinals(&mut served);
    let loaded = store.load(SESSION).unwrap();
    let m0 = loaded
        .core
        .frozen_units
        .iter()
        .find(|unit| unit.key == "m0")
        .map(|unit| unit.frozen_payload.clone());
    let tags: Vec<Value> = store
        .load_tags_for_session(SESSION)
        .unwrap()
        .into_iter()
        .map(|row| json!([row.tag_number, row.block_id, row.kind]))
        .collect();
    json!({
        "op": op.name(),
        "status": response["status"],
        "code": response["code"],
        "action": response["action"],
        "served": served,
        "m0": m0,
        "tags": tags,
    })
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "records revision 2 goldens; run on a revision 2 daemon, see the module docs"]
async fn record_revision_2_goldens() {
    if std::env::var("EIDNARA_RECORD_REV2_GOLDENS").as_deref() != Ok("1") {
        return;
    }
    let mut recorded = Vec::new();
    for (name, ops) in traces() {
        let (handler, store, _dir) = golden_handler();
        let mut host = Host::default();
        let mut steps = Vec::new();
        for op in ops.iter().copied() {
            host.apply(op, &store);
            let mut body = pass_body(host.wire(0, true));
            body["v"] = json!(2);
            let response = call(&handler, body).await;
            steps.push(step(op, &response, &store));
        }
        recorded.push(json!({ "trace": name, "steps": steps }));
    }
    let mut text = serde_json::to_string_pretty(&Value::Array(recorded)).unwrap();
    text.push('\n');
    std::fs::write(GOLDENS_PATH, text).unwrap();
}

/// The revision 3 plugin's per-session boundary: `None` until discovered, then the last
/// response's rendered boundary (`Some(None)` sends the whole array).
#[derive(Default)]
struct Plugin {
    boundary: Option<Option<Value>>,
}

impl Plugin {
    /// Walks `transform.boundary` newest first and declares the first anchor the host holds;
    /// an exhausted walk declares nothing.
    async fn discover(&mut self, handler: &Handler, host: &Host) {
        let mut before = None::<i64>;
        self.boundary = Some(loop {
            let mut request =
                json!({ "method": "transform.boundary", "v": 3, "session_id": SESSION });
            if let Some(before) = before {
                request["before_sequence"] = json!(before);
            }
            let PreparedOutcome::Response(bytes) =
                handler.dispatch_value(test_route(7), request).await
            else {
                panic!("discovery answers a page");
            };
            let page: Value = serde_json::from_slice(&bytes).unwrap();
            let anchors = page["anchors"].as_array().unwrap().clone();
            let Some(last) = anchors.last() else {
                break None;
            };
            if let Some(found) = anchors
                .iter()
                .find(|found| host.messages.iter().any(|(mid, _, _)| found["mid"] == *mid))
            {
                break Some(found.clone());
            }
            before = last["sequence"].as_i64();
        });
    }

    /// One pass: the window from the boundary when the host holds it, else one discovery.
    async fn pass(&mut self, handler: &Handler, host: &Host) -> Value {
        let mut discovered = false;
        loop {
            let at = match &self.boundary {
                Some(Some(anchor)) => host
                    .messages
                    .iter()
                    .position(|(mid, _, _)| anchor["mid"] == *mid),
                Some(None) => Some(0),
                None => None,
            };
            let Some(at) = at.filter(|_| !discovered || self.boundary.is_some()) else {
                if discovered {
                    return json!({ "status": "declined" });
                }
                self.discover(handler, host).await;
                discovered = true;
                continue;
            };
            let declared = self.boundary.clone().flatten();
            let at = if declared.is_some() { at } else { 0 };
            let mut body = pass_body(host.wire(at, false));
            body["v"] = json!(3);
            body["boundary"] = declared.unwrap_or(Value::Null);
            let response = call(handler, body).await;
            match response["status"].as_str() {
                Some("ok") => {
                    self.boundary = Some(
                        response["boundary"]
                            .as_object()
                            .map(|_| response["boundary"].clone()),
                    )
                }
                Some("boundary_unknown") if !discovered => {
                    self.discover(handler, host).await;
                    discovered = true;
                    continue;
                }
                _ => {}
            }
            return response;
        }
    }
}

/// The revision 2 goldens replayed against revision 3: after every operation of every trace,
/// the served array without ordinals, the m0 bytes, and the tag rows equal the recorded ones.
/// Where revision 2 served a revert before the first anchor raw and kept doing so, revision 3
/// resets the session (spec D10), so from that step on every step equals a fresh session's step
/// over the same host arrays instead, tag numbers aside. Where revision 2 refused an edit of a
/// covered message (covered-drift rejection, deleted by spec D25) and every later pass of the
/// trace with `transform_failed`, revision 3 serves, and the goldens have no output to compare
/// until such a reset.
#[tokio::test(flavor = "current_thread")]
async fn revision_3_replays_the_revision_2_goldens() {
    let goldens: Value =
        serde_json::from_str(&std::fs::read_to_string(GOLDENS_PATH).unwrap()).unwrap();
    let goldens = goldens.as_array().unwrap();
    let traces = traces();
    assert_eq!(goldens.len(), traces.len());
    for ((name, ops), golden) in traces.into_iter().zip(goldens) {
        assert_eq!(golden["trace"], name.as_str());
        let (handler, store, _dir) = golden_handler();
        let mut host = Host::default();
        let mut plugin = Plugin::default();
        let mut fresh = None;
        let mut refused = false;
        let mut tags_before = Vec::new();
        let mut pre_reset_tags = Vec::new();
        for (index, op) in ops.iter().copied().enumerate() {
            host.apply(op, &store);
            let response = plugin.pass(&handler, &host).await;
            let actual = step(op, &response, &store);
            if golden["steps"][index]["action"] == "PASSTHROUGH" && fresh.is_none() {
                fresh = Some((golden_handler(), Plugin::default()));
                pre_reset_tags = std::mem::take(&mut tags_before);
            }
            tags_before = actual["tags"].as_array().unwrap().clone();
            refused |= golden["steps"][index]["code"] == "transform_failed";
            if refused && fresh.is_none() {
                // Covered drift is served (spec D25): the host messages after the boundary.
                assert_eq!(actual["status"], "ok", "{name} step {index} ({op:?})");
                assert_eq!(
                    served_mids(&actual["served"]),
                    tail_mids(&host, &response["boundary"]),
                    "{name} step {index} ({op:?}) served"
                );
                continue;
            }
            let expected = match fresh.as_mut() {
                Some(((fresh_handler, fresh_store, _), fresh_plugin)) => {
                    let response = fresh_plugin.pass(fresh_handler, &host).await;
                    &step(op, &response, fresh_store)
                }
                None => &golden["steps"][index],
            };
            for field in ["op", "status", "code", "action", "served", "m0", "tags"] {
                if fresh.is_some() && field == "tags" {
                    // Tag rows are session-wide and outlive the reset, so tag numbers continue.
                    assert_minted_after_reset(
                        &pre_reset_tags,
                        &actual[field],
                        &expected[field],
                        &format!("{name} step {index} ({op:?})"),
                    );
                    continue;
                }
                if fresh.is_some() && field == "served" {
                    assert_eq!(
                        untagged(&actual[field]),
                        untagged(&expected[field]),
                        "{name} step {index} ({op:?}) {field}"
                    );
                    continue;
                }
                assert_eq!(
                    actual[field], expected[field],
                    "{name} step {index} ({op:?}) {field}"
                );
            }
        }
    }
}

/// After a reset the pre-reset tag rows stay as they were, and the rows minted since are what a
/// fresh session mints for blocks the pre-reset rows do not already tag, numbered uniquely above
/// every pre-reset number.
fn assert_minted_after_reset(pre: &[Value], actual: &Value, fresh: &Value, at: &str) {
    let actual = actual.as_array().unwrap();
    assert!(pre.iter().all(|row| actual.contains(row)), "{at} tags");
    let minted: Vec<&Value> = actual.iter().filter(|row| !pre.contains(row)).collect();
    let floor = pre
        .iter()
        .filter_map(|row| row[0].as_i64())
        .max()
        .unwrap_or(0);
    let numbers: HashSet<i64> = minted.iter().filter_map(|row| row[0].as_i64()).collect();
    assert_eq!(numbers.len(), minted.len(), "{at} tag numbers");
    assert!(numbers.iter().all(|n| *n > floor), "{at} tag numbers");
    let blank = |row: &Value| json!([row[1], row[2]]);
    let tagged: Vec<Value> = pre.iter().map(blank).collect();
    let sorted = |mut rows: Vec<Value>| {
        rows.sort_by_key(Value::to_string);
        rows
    };
    let expected = fresh
        .as_array()
        .unwrap()
        .iter()
        .map(blank)
        .filter(|row| !tagged.contains(row))
        .collect();
    assert_eq!(
        sorted(minted.into_iter().map(blank).collect()),
        sorted(expected),
        "{at} tags"
    );
}

/// The harness ids of a served array's non-synthetic messages.
fn served_mids(served: &Value) -> Vec<String> {
    served
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["meta"]["synthetic"] != true)
        .map(|message| message["meta"]["harness_id"].as_str().unwrap().to_string())
        .collect()
}

/// The host's mids after `boundary`, or all of them under a `null` boundary.
fn tail_mids(host: &Host, boundary: &Value) -> Vec<String> {
    let after = if boundary.is_null() {
        0
    } else {
        host.messages
            .iter()
            .position(|(mid, _, _)| boundary["mid"] == *mid)
            .expect("the boundary mid is in the host array")
            + 1
    };
    host.messages[after..]
        .iter()
        .map(|(mid, _, _)| mid.clone())
        .collect()
}

/// `served` as JSON with every `§<digits>§` tag number blanked.
fn untagged(served: &Value) -> String {
    regex::Regex::new("§[0-9]+§")
        .unwrap()
        .replace_all(&served.to_string(), "§§")
        .into_owned()
}
