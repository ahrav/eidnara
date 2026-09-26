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
/// removes every anchor, only appends and edits follow, since the session serves raw from then on.
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
